// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the one relation `StartQuery` holds between the time range a query is
//! asked over and the log groups that query reads: a range that CLOSES before a log group came
//! into existence.
//!
//! The rule under test: a query whose `endTime` lies before the creation instant of a log group
//! it reads covers no instant of that group's lifetime, so the request is refused with HTTP 400
//! `MalformedQueryException` and NO query is created — `DescribeQueries` reports nothing for the
//! group afterwards. The range's end is inclusive ("the range is inclusive, so the specified end
//! time is included in the query", `StartQuery$endTime`, in "epoch time, the number of seconds
//! since January 1, 1970"), so a range ending exactly at the instant the group was created
//! covers that instant and is served; where the range OPENS is free, and a range opening long
//! before the group existed still reads the events the group holds in it.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so an answer a
//! handler does not compute from the shared store cannot pass.
//!
//! The tables span what the rule generalises over rather than the one request an issue cites:
//! how far before the creation instant the range closes (one second, minutes, an hour, a day,
//! and down at `endTime`'s published minimum of 0), which of the three members names the log
//! groups, how many groups the request names and whether only one of them is younger than the
//! range, and — for what is SERVED — the range that ends exactly at the creation instant, one
//! second past it, and one in the future.
//!
//! The groups judged are the ones the REQUEST names, which is where `StartQuery` resolves them;
//! a query whose own `SOURCE` command selects its log groups by prefix names none, and selects
//! whatever the account holds under that prefix when the query runs.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::QUERY_STATUS_COMPLETE;

use crate::harness::{batch, now_ms, without_sdk_envelope, Logs};

/// The message a refusal of this class carries, as the recorded refusal words it: the
/// sentence, then the marker that stands where a fault IN THE QUERY STRING would have its
/// character span. Spelled out here rather than taken from the service, so the wording itself
/// is what the cases below pin.
const RECORDED_MESSAGE: &str = "Query's end date and time is either before the log groups \
                                creation time or exceeds the log groups log retention settings \
                                <no span>";

/// The operations under test, and the reads a refusal and an acceptance are proved through.
const START: &str = "StartQuery";
const DESCRIBE: &str = "DescribeQueries";
const RESULTS: &str = "GetQueryResults";

/// The status and the code a range that closes before a read log group existed is refused
/// under: `MalformedQueryException` is modelled a client error for this operation
/// (`smithy_query(query="operation",operation="StartQuery")`), which the response answers 400.
const REFUSAL_STATUS: u16 = 400;
const MALFORMED: &str = "MalformedQueryException";

/// A query string that compiles, so no case is refused for its text.
const VALID_QUERY: &str = "fields @timestamp, @message";

/// The log groups the cases query.
const GROUP: &str = "/window-admission/one";
const OTHER_GROUP: &str = "/window-admission/two";

/// How long to wait, at most, for a started query to reach a terminal status — generous, because
/// it bounds a POLL for the status rather than the time the run is expected to take.
const COMPLETION_DEADLINE: Duration = Duration::from_secs(30);

/// How long between two looks at a started query's status.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The epoch SECOND a range must close at or after to cover `creation_ms`: the creation instant
/// rounded UP to a whole second, since the range is stated in whole seconds and its end is
/// inclusive.
fn creation_second(creation_ms: i64) -> i64 {
    let whole = creation_ms / 1_000;
    if creation_ms % 1_000 == 0 {
        whole
    } else {
        whole + 1
    }
}

/// The `creationTime` the service reports for `group`, in the epoch milliseconds
/// `LogGroup$creationTime` carries.
async fn group_creation_ms(logs: &Logs, group: &str) -> i64 {
    let reported = logs
        .describe_group(group)
        .await
        .unwrap_or_else(|| panic!("the log group {group} must be reported once created"));
    reported
        .get("creationTime")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("a log group carries creationTime, got {reported}"))
}

/// Create both log groups and answer the epoch second the younger one's creation instant falls
/// in, rounded up.
async fn groups(logs: &Logs) -> i64 {
    let mut latest = 0;
    for group in [GROUP, OTHER_GROUP] {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
        latest = latest.max(group_creation_ms(logs, group).await);
    }
    creation_second(latest)
}

/// The queries `DescribeQueries` reports for `filter`.
async fn described(logs: &Logs, filter: &Value) -> Vec<Value> {
    let page = logs.ok(DESCRIBE, filter).await;
    page["queries"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeQueries reports a queries list, got {page}"))
        .clone()
}

/// A range whose end lies before a log group it reads was created is refused as a malformed
/// query, in the service's own sentence, and leaves no query behind.
#[tokio::test]
async fn a_range_closing_before_a_read_group_existed_is_refused_and_creates_no_query() {
    let logs = Logs::new();
    let created = groups(&logs).await;
    let (_, group_arn) = logs.group_arns(GROUP).await;
    let day = 24 * 60 * 60;

    // (case, startTime, endTime, how the request names the log groups to query). Every end is
    // before the second the groups were created in, so every row is the same condition reached
    // by a different route.
    let cases: Vec<(&str, i64, i64, Value)> = vec![
        (
            "a range closing one second before the group existed",
            created - 60,
            created - 1,
            json!({"logGroupName": GROUP}),
        ),
        (
            "a range closing twenty minutes before the group existed",
            created - 40 * 60,
            created - 20 * 60,
            json!({"logGroupName": GROUP}),
        ),
        (
            "a range closing an hour before the group existed",
            created - 2 * 60 * 60,
            created - 60 * 60,
            json!({"logGroupNames": [GROUP]}),
        ),
        (
            "a range closing a day before the group existed",
            created - 2 * day,
            created - day,
            json!({"logGroupNames": [GROUP, OTHER_GROUP]}),
        ),
        (
            // 0 is the published minimum of both ends' `@range`, so this is the widest range
            // the model admits that still closes before the group existed.
            "a range of a single instant at the published minimum of the end",
            0,
            0,
            json!({"logGroupName": GROUP}),
        ),
        (
            "a range closing before the group existed, named by the group's ARN",
            0,
            created - 1,
            json!({"logGroupIdentifiers": [group_arn.clone()]}),
        ),
    ];
    for (case, start_time, end_time, selector) in cases {
        // Every row's selector names the log groups by a MEMBER, so the query string is the
        // one that compiles and is never what a row varies.
        let mut request = json!({
            "startTime": start_time,
            "endTime": end_time,
            "queryString": VALID_QUERY,
        });
        for (member, value) in selector.as_object().expect("the selector is an object") {
            request[member] = value.clone();
        }

        let (status, body) = logs.call(START, &request).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
        );
        let code = crate::harness::error_code(&body);
        assert_eq!(
            code, MALFORMED,
            "case {case:?}: expected {MALFORMED}, got {code} with body {body}"
        );
        let said = body["message"].as_str().unwrap_or_default();
        // The wording is pinned LITERALLY here, so a message that drifted from the one this
        // class is refused with would fail even if the service changed its own constant: the
        // text is fixed for the condition, carrying neither end of the submitted range nor
        // the name of any group.
        assert_eq!(
            without_sdk_envelope(case, said, REFUSAL_STATUS, MALFORMED),
            RECORDED_MESSAGE,
            "case {case:?}: expected {RECORDED_MESSAGE:?}, got {said:?}"
        );
        // The fault is in the RANGE, so there is no position in the query string to point at.
        assert!(
            body["queryCompileError"].is_null(),
            "case {case:?}: the range, not the query string, is at fault, got {body}"
        );
    }

    // A refused start creates nothing: no query is reported for either group, or at all.
    for filter in [
        json!({}),
        json!({"logGroupName": GROUP}),
        json!({"logGroupName": OTHER_GROUP}),
    ] {
        let reported = described(&logs, &filter).await;
        assert!(
            reported.is_empty(),
            "case the filter {filter}: a refused start creates no query, got {reported:?}"
        );
    }
}

/// One group younger than the range's end is enough: a request naming it beside an older group
/// is refused, and the same range over the older group alone is served.
#[tokio::test]
async fn a_range_is_judged_against_every_group_the_query_reads() {
    let logs = Logs::new();
    const OLDER: &str = "/window-admission/older";
    logs.ok("CreateLogGroup", &json!({"logGroupName": OLDER}))
        .await;
    let older_created = creation_second(group_creation_ms(&logs, OLDER).await);
    // A range that covers the older group's whole lifetime, and closes before the younger group
    // below is created.
    let end_time = older_created.max(now_ms() / 1_000 + 1);

    const YOUNGER: &str = "/window-admission/younger";
    // The younger group is created strictly after the range's end: whole seconds, so waiting for
    // the clock to leave the second the end names is what makes it so.
    let deadline = std::time::Instant::now() + COMPLETION_DEADLINE;
    while now_ms() / 1_000 <= end_time {
        assert!(
            std::time::Instant::now() < deadline,
            "the clock must pass the second the range ends in"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    logs.ok("CreateLogGroup", &json!({"logGroupName": YOUNGER}))
        .await;

    // (case, the groups the request names, whether the start is served)
    let cases: Vec<(&str, Vec<&str>, bool)> = vec![
        ("the older group alone", vec![OLDER], true),
        ("the younger group alone", vec![YOUNGER], false),
        (
            "the older group beside the younger one",
            vec![OLDER, YOUNGER],
            false,
        ),
        (
            "the younger group beside the older one",
            vec![YOUNGER, OLDER],
            false,
        ),
    ];
    for (case, names, served) in cases {
        let (status, body) = logs
            .call(
                START,
                &json!({
                    "logGroupNames": names,
                    "startTime": 0,
                    "endTime": end_time,
                    "queryString": VALID_QUERY,
                }),
            )
            .await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            assert!(
                body["queryId"].as_str().is_some_and(|id| !id.is_empty()),
                "case {case:?}: a served start answers a queryId, got {body}"
            );
        } else {
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
            );
            let code = crate::harness::error_code(&body);
            assert_eq!(
                code, MALFORMED,
                "case {case:?}: expected {MALFORMED}, got {code} with body {body}"
            );
        }
    }
}

/// What the rule SERVES: a range reaching the creation instant or beyond, however far back it
/// opens — and the events it then reads are the ones the group holds in it, including events
/// stamped before the group itself was created.
#[tokio::test]
async fn a_range_reaching_the_creation_instant_is_served_and_reads_the_events_in_it() {
    let logs = Logs::new();
    const GROUP_WITH_EVENTS: &str = "/window-admission/served";
    const STREAM: &str = "stream-one";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP_WITH_EVENTS}))
        .await;
    let created = group_creation_ms(&logs, GROUP_WITH_EVENTS).await;
    let created_second = creation_second(created);
    logs.ok(
        "CreateLogStream",
        &json!({"logGroupName": GROUP_WITH_EVENTS, "logStreamName": STREAM}),
    )
    .await;
    // An event stamped BEFORE the group was created, which is what the served range reads: the
    // range bounds which events a query reads, and this one predates the group.
    const BACKDATED: &str = "BACKDATED order 1 failed";
    let backdated_ms = created - 25 * 60 * 1_000;
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": GROUP_WITH_EVENTS,
            "logStreamName": STREAM,
            "logEvents": batch(&[(backdated_ms, BACKDATED)]),
        }),
    )
    .await;

    // (case, startTime, endTime) — each range reaches the creation instant or beyond, and each
    // opens before the backdated event.
    let cases: [(&str, i64, i64); 4] = [
        (
            "a range ending exactly at the instant the group was created",
            created_second - 60 * 60,
            created_second,
        ),
        (
            "a range ending one second past the creation instant",
            created_second - 60 * 60,
            created_second + 1,
        ),
        (
            "a range opening at the published minimum of the range",
            0,
            created_second + 1,
        ),
        (
            "a range ending two minutes in the future",
            created_second - 40 * 60,
            created_second + 120,
        ),
    ];
    for (case, start_time, end_time) in cases {
        let (status, body) = logs
            .call(
                START,
                &json!({
                    "logGroupName": GROUP_WITH_EVENTS,
                    "startTime": start_time,
                    "endTime": end_time,
                    "queryString": VALID_QUERY,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let id = body["queryId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a served start answers a queryId, got {body}"))
            .to_string();
        // The served query is in the store, not merely acknowledged.
        assert!(
            described(&logs, &json!({"logGroupName": GROUP_WITH_EVENTS}))
                .await
                .iter()
                .any(|query| query["queryId"].as_str() == Some(id.as_str())),
            "case {case:?}: the started query must be reported under {id}"
        );

        // Wait for the CONDITION — the query reaching its terminal status — rather than for a
        // span of the clock.
        let deadline = std::time::Instant::now() + COMPLETION_DEADLINE;
        let answer = loop {
            let answer = logs.ok(RESULTS, &json!({"queryId": id})).await;
            if answer["status"].as_str() == Some(QUERY_STATUS_COMPLETE) {
                break answer;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "case {case:?}: the query must reach {QUERY_STATUS_COMPLETE}, got {answer}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        let messages: Vec<String> = answer["results"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the results are a list, got {answer}"))
            .iter()
            .flat_map(|row| {
                row.as_array()
                    .map(|fields| fields.to_vec())
                    .unwrap_or_default()
            })
            .filter(|field| field["field"].as_str() == Some("@message"))
            .filter_map(|field| field["value"].as_str().map(ToString::to_string))
            .collect();
        assert_eq!(
            messages,
            vec![BACKDATED.to_string()],
            "case {case:?}: the served range reads the events it covers, got {answer}"
        );
    }
}
