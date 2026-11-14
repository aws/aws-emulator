// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal `StartQuery` answers when the window it is asked to
//! query over runs BACKWARDS — its `endTime` before its `startTime`.
//!
//! The rule under test: a Logs Insights query whose end time precedes its start time spans
//! no instant, so it is refused `InvalidParameterException` (HTTP 400) whose message is the
//! service's fixed sentence for that condition followed by the AWS SDK error envelope —
//! `(Service: AWSLogs; Status Code: <the status answered>; Error Code: <the code
//! answered>; Request ID: <this response's own request id>; Proxy: null)` — with neither
//! submitted timestamp interpolated anywhere in it, and the refused call schedules nothing.
//!
//! The table spans what the rule generalises over: how far apart the two ends are (one
//! second to a whole day), where in time the window sits (the past, the present, the
//! future, and down at `startTime`/`endTime`'s published minimum of 0), and which member
//! names the log groups being queried — the recorded hour-long inversion is one row.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response a handler does not actually compute cannot pass.

use serde_json::{json, Value};

use crate::harness::{envelope_request_id, now_ms, Logs};

/// The operations under test.
const START: &str = "StartQuery";
const DESCRIBE: &str = "DescribeQueries";

/// The modelled error code an incorrectly specified parameter is refused under
/// (`smithy_query(query="operation",operation="StartQuery")`: `InvalidParameterException`,
/// client error, HTTP 400).
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The status such a refusal is answered with.
const REFUSAL_STATUS: u16 = 400;

/// A query string that compiles, so no case below is refused over its query syntax.
const VALID_QUERY: &str = "fields @message";

/// The message the rule requires, for a refusal whose envelope reports `request_id`.
fn expected_message(request_id: &str) -> String {
    format!(
        "End time cannot be less than Start time (Service: AWSLogs; Status Code: \
         {REFUSAL_STATUS}; Error Code: {INVALID_PARAMETER}; Request ID: {request_id}; \
         Proxy: null)"
    )
}

/// The message a refusal reports.
fn message(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a refusal carries a message, got {body}"))
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

#[tokio::test]
async fn an_inverted_query_window_is_refused_in_the_services_own_wording() {
    let logs = Logs::new();
    const GROUP: &str = "/windows/inverted";
    const OTHER_GROUP: &str = "/windows/inverted-two";
    for group in [GROUP, OTHER_GROUP] {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
    let (_, group_arn) = logs.group_arns(GROUP).await;
    // The window is in the unit `StartQuery` documents for its range: "epoch time, the
    // number of SECONDS since January 1, 1970, 00:00:00 UTC".
    let now_secs = now_ms() / 1_000;
    let day = 24 * 60 * 60;

    // (case, startTime, endTime, how the request names the log groups to query)
    let cases: Vec<(&str, i64, i64, Value)> = vec![
        (
            "the ends an hour apart, swapped",
            now_secs,
            now_secs - 60 * 60,
            json!({"logGroupName": GROUP}),
        ),
        (
            "an endTime one second before the startTime",
            now_secs,
            now_secs - 1,
            json!({"logGroupName": GROUP}),
        ),
        (
            "a window inverted by a whole day",
            now_secs,
            now_secs - day,
            json!({"logGroupNames": [GROUP]}),
        ),
        (
            "a window wholly in the past, inverted",
            now_secs - day,
            now_secs - 2 * day,
            json!({"logGroupNames": [GROUP, OTHER_GROUP]}),
        ),
        (
            "a window wholly in the future, inverted",
            now_secs + 2 * day,
            now_secs + day,
            json!({"logGroupIdentifiers": [group_arn.clone()]}),
        ),
        (
            // 0 is the published minimum of both members' `@range`, so this is the widest
            // inversion the model admits at all.
            "an endTime at the published minimum of the range",
            now_secs,
            0,
            json!({"logGroupName": GROUP}),
        ),
        (
            "both ends one second apart at the published minimum of the range",
            1,
            0,
            json!({"logGroupName": GROUP}),
        ),
        (
            // The log groups named by the query's own SOURCE command instead of a member,
            // so the refusal does not depend on which selector was used.
            "a SOURCE command selecting the groups",
            now_secs,
            now_secs - 60,
            json!({
                "queryString": r#"SOURCE logGroups(namePrefix: ["/windows/"]) | fields @message"#,
            }),
        ),
    ];

    let mut seen_request_ids: Vec<String> = Vec::new();
    for (case, start_time, end_time, selector) in cases {
        let mut request = json!({"startTime": start_time, "endTime": end_time});
        if selector.get("queryString").is_none() {
            request["queryString"] = json!(VALID_QUERY);
        }
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
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} with body {body}"
        );

        let said = message(case, &body);
        let request_id = envelope_request_id(case, &said);
        // Equality against the template is also what proves the sentence is FIXED for the
        // condition: the template carries no value from the request, so a message that
        // echoed either submitted end could not match it.
        assert_eq!(
            said,
            expected_message(&request_id),
            "case {case:?}: expected {:?}, got {said:?}",
            expected_message(&request_id)
        );
        // A request id identifies ONE response, so no two refusals report the same one.
        assert!(
            !seen_request_ids.contains(&request_id),
            "case {case:?}: every refusal carries its own request id, got the \
             already-reported {request_id:?}"
        );
        seen_request_ids.push(request_id);
    }

    // A refused start schedules nothing: nothing is reported for either group, or at all.
    for filter in [
        json!({}),
        json!({"logGroupName": GROUP}),
        json!({"logGroupName": OTHER_GROUP}),
    ] {
        let reported = described(&logs, &filter).await;
        assert!(
            reported.is_empty(),
            "case the filter {filter}: a refused start schedules no query, got {reported:?}"
        );
    }

    // The positive control: a window that is a range — forward, and of a single instant — is
    // accepted, and the query it starts is then reported. Every accepted end is rounded UP to
    // the next whole second past the present, so the range reaches the instant the log groups
    // were created.
    let horizon = now_secs + 1;
    let accepted: [(&str, i64, i64); 3] = [
        ("a forward window", now_secs - 60 * 60, horizon),
        ("a window of a single instant", horizon, horizon),
        ("a forward window opening at the minimum of the range", 0, horizon),
    ];
    for (case, start_time, end_time) in accepted {
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
        let id = answer["queryId"].as_str().unwrap_or_default().to_string();
        assert!(
            !id.is_empty(),
            "case {case:?}: an accepted start answers a queryId, got {answer}"
        );
        assert!(
            described(&logs, &json!({"logGroupName": GROUP}))
                .await
                .iter()
                .any(|query| query["queryId"].as_str() == Some(id.as_str())),
            "case {case:?}: the started query must be reported under {id}"
        );
    }
}
