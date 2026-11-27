// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal message a query operation of this service answers: the
//! request its envelope STATES, against the one the response that carries it REPORTS, and the
//! parser diagnostic a query that does not compile is refused with.
//!
//! The rules under test:
//!
//! * A response names ONE request. CloudWatch Logs words a refusal IT DECIDES as the sentence
//!   for its class followed by the AWS SDK error envelope — ` (Service: AWSLogs; Status Code:
//!   <the status answered>; Error Code: <the code answered>; Request ID: <id>; Proxy: null)` —
//!   and the id stated in that clause is the id the response reports in the header this
//!   protocol names a request in (`x-amzn-RequestId`), freshly minted per call. So a client
//!   that reads the id out of an SDK's error message and a client that reads the response
//!   header see the same value for one call, whichever refusal class answered.
//! * A refusal this service RELAYS rather than decides — the query compiler's account of a
//!   query that does not compile — carries the compiler's text as its SENTENCE, and then that
//!   same envelope: the sentence is the compiler's diagnostic, the symbol it stopped on and the
//!   place that symbol occupies in the SUBMITTED query string, and the modelled
//!   `queryCompileError` detail beside it carries that diagnostic alone.
//!
//! The rows span what the rules generalise over: which error class the refusal is (so the
//! status and code stated in the clause differ), which operation answers it, whether the
//! refusal is decided over the time range, a named resource or a stored id, where in the query
//! string the offending token sits and on which line, and whether the submitted text itself
//! carries the marker a message is worded with — a quoted symbol is left exactly as the handler
//! wrote it, and only a clause the service words states a response's id.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a response
//! a handler does not actually compute cannot pass.

use serde_json::{json, Value};

use crate::harness::{now_ms, Logs, REQUEST_ID_PLACEHOLDER};

/// The header this protocol names the request a response answers in.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The status every refusal below is answered with.
const REFUSAL_STATUS: u16 = 400;

/// The error classes the rows below are refused under, each declared by the operation that
/// answers it (`smithy_query(query="operation",operation="StartQuery")` /
/// `...,operation="PutQueryDefinition"`).
const MALFORMED_QUERY: &str = "MalformedQueryException";
/// See [`MALFORMED_QUERY`].
const INVALID_PARAMETER: &str = "InvalidParameterException";
/// See [`MALFORMED_QUERY`].
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The id a response reports for the request it answers, asserted to be there and to carry the
/// 8-4-4-4-12 lowercase-hex form an AWS request id has.
fn reported_request_id(case: &str, headers: &http::HeaderMap) -> String {
    let stated = headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| panic!("case {case}: the response names the request it answers"))
        .to_string();
    let groups: Vec<&str> = stated.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        vec![8usize, 4, 4, 4, 12],
        "case {case}: expected an 8-4-4-4-12 request id, got {stated:?}"
    );
    assert!(
        groups.iter().all(|group| group
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())),
        "case {case}: expected lowercase hex in the request id, got {stated:?}"
    );
    stated
}

/// One refused call: the status, the parsed body, the raw body text and the id the response
/// reports for the request.
async fn refused(
    logs: &Logs,
    case: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value, String, String) {
    let (status, headers, payload) = logs.call_raw(operation, request).await;
    let text = String::from_utf8(payload.to_vec())
        .unwrap_or_else(|_| panic!("case {case}: the error body is text, got {payload:?}"));
    let body: Value = serde_json::from_str(&text)
        .unwrap_or_else(|_| panic!("case {case}: the error body is JSON, got {text:?}"));
    let request_id = reported_request_id(case, &headers);
    (status, body, text, request_id)
}

/// The message a refusal reports.
fn message(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: a refusal carries a message, got {body}"))
        .to_string()
}

/// The envelope clause the rule requires: the status and the code the response itself answers
/// with, and the request the clause names.
fn expected_clause(code: &str, request_id: &str) -> String {
    format!(
        " (Service: AWSLogs; Status Code: {REFUSAL_STATUS}; Error Code: {code}; Request ID: \
         {request_id}; Proxy: null)"
    )
}

/// A window in the unit `StartQuery` documents its range in ("epoch time, the number of
/// seconds since January 1, 1970"), an hour wide and ending now.
fn window() -> (i64, i64) {
    let now_secs = now_ms() / 1_000 + 1;
    (now_secs - 60 * 60, now_secs)
}

/// Every refusal class this service DECIDES states the id the response that carries it reports.
#[tokio::test]
async fn every_decided_refusal_clause_states_the_id_the_response_reports() {
    let logs = Logs::new();
    const GROUP: &str = "/envelope/one";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let (start, end) = window();

    // (case, operation, request, the error class it is refused under)
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a query window that runs backwards",
            "StartQuery",
            json!({
                "logGroupName": GROUP,
                "startTime": end,
                "endTime": start,
                "queryString": "fields @message",
            }),
            INVALID_PARAMETER,
        ),
        (
            "a query over a log group this account does not hold",
            "StartQuery",
            json!({
                "logGroupName": "/envelope/absent",
                "startTime": start,
                "endTime": end,
                "queryString": "fields @message",
            }),
            NOT_FOUND,
        ),
        (
            // The range closes before the group it reads was created, which is a fault of the
            // range rather than of the query text, so the service decides it itself.
            "a query window that closes before the log group existed",
            "StartQuery",
            json!({
                "logGroupName": GROUP,
                "startTime": start - 10 * 24 * 60 * 60,
                "endTime": start - 9 * 24 * 60 * 60,
                "queryString": "fields @message",
            }),
            MALFORMED_QUERY,
        ),
        (
            "a query definition id this account holds none under, on a put",
            "PutQueryDefinition",
            json!({
                "name": "envelope/put",
                "queryString": "fields @message",
                "queryDefinitionId": "9ca5cd49-0c22-4b56-91b0-1f0e3f2ba7a1",
            }),
            NOT_FOUND,
        ),
        (
            "a query definition id this account holds none under, on a delete",
            "DeleteQueryDefinition",
            json!({"queryDefinitionId": "1e7a6f43-5d9b-4a8c-8a4d-2f6f9e0b1c23"}),
            NOT_FOUND,
        ),
    ];

    for (case, operation, request, code) in cases {
        let (status, body, text, request_id) = refused(&logs, case, operation, &request).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case}: expected the refusal status, got {status} with {text}"
        );
        assert_eq!(
            crate::harness::error_code(&body),
            code,
            "case {case}: expected {code}, got {text}"
        );
        let said = message(case, &body);
        let clause = expected_clause(code, &request_id);
        assert!(
            said.ends_with(&clause),
            "case {case}: expected the clause {clause:?} stating the reported id, got {said:?}"
        );
        // Nothing of the marker a message is worded with survives anywhere in the response.
        assert!(
            !text.contains(REQUEST_ID_PLACEHOLDER),
            "case {case}: expected no unsubstituted marker, got {text}"
        );
    }
}

/// The id is per RESPONSE: two identical refused calls state two different ids, and each states
/// the one its OWN response reports.
#[tokio::test]
async fn each_decided_refusal_states_its_own_responses_id() {
    let logs = Logs::new();
    const GROUP: &str = "/envelope/per-call";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let (start, end) = window();
    // A window that runs backwards: a refusal this service decides itself, so its clause names
    // the response's own request.
    let request = json!({
        "logGroupName": GROUP,
        "startTime": end,
        "endTime": start,
        "queryString": "fields @message",
    });

    let mut stated: Vec<String> = Vec::new();
    for attempt in 0..4 {
        let case = format!("call {attempt}");
        let (status, body, text, request_id) =
            refused(&logs, &case, "StartQuery", &request).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case}: expected the refusal status, got {status} with {text}"
        );
        let said = message(&case, &body);
        let clause = expected_clause(INVALID_PARAMETER, &request_id);
        assert!(
            said.ends_with(&clause),
            "case {case}: expected the clause {clause:?} stating this response's id, got \
             {said:?}"
        );
        stated.push(request_id);
    }
    let mut distinct = stated.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        stated.len(),
        "expected one id per response, got {stated:?}"
    );
}

/// What the substitution does NOT touch: the symbol a parser diagnostic quotes out of the
/// submitted query string is reported exactly as it was submitted, even when it is the very
/// text a message is worded with.
#[tokio::test]
async fn a_quoted_symbol_carrying_the_marker_is_reported_as_submitted() {
    let logs = Logs::new();
    const GROUP: &str = "/envelope/echo";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let (start, end) = window();

    // (case, the query string, the symbol the compiler stops on)
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "the marker as the command of the second stage",
            format!("fields @message | {REQUEST_ID_PLACEHOLDER}"),
            REQUEST_ID_PLACEHOLDER,
        ),
        (
            "the marker inside a longer unknown command",
            format!("fields @message | sort @timestamp | a{REQUEST_ID_PLACEHOLDER}b"),
            "a{request-id}b",
        ),
    ];

    for (case, query_string, symbol) in cases {
        let request = json!({
            "logGroupName": GROUP,
            "startTime": start,
            "endTime": end,
            "queryString": query_string,
        });
        let (status, body, text, _) = refused(&logs, case, "StartQuery", &request).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case}: expected the refusal status, got {status} with {text}"
        );
        let said = message(case, &body);
        let position = query_string
            .find(symbol)
            .unwrap_or_else(|| panic!("case {case}: the row's query carries {symbol:?}"));
        // The clause the message closes with is the service's own; the SENTENCE in front of it
        // is where the submitted symbol is quoted, and the marker inside the quoted symbol is
        // left exactly as it was submitted.
        let sentence =
            crate::harness::without_sdk_envelope(case, &said, REFUSAL_STATUS, MALFORMED_QUERY);
        assert_eq!(
            sentence,
            format!("unexpected symbol found {symbol} at line 1 and position {position}"),
            "case {case}: expected the submitted symbol quoted in the diagnostic, got {said:?}"
        );
    }
}

/// The WHOLE message of a query that does not compile: the parser diagnostic — the symbol the
/// compiler stopped on and the place that symbol occupies in the SUBMITTED query string — and
/// then the envelope clause every refusal of this service closes with, naming the request this
/// response answers.
///
/// The rows span what the message's variable parts generalise over: where the offending token
/// sits (so the position moves with it, by exactly as many characters as the token moved), what
/// the token is (a word, a single character, a token carrying punctuation), which line of a
/// multi-line query string it is on, and how long the pipeline before it is. The place is read
/// off the submitted string in each row rather than restated, so a diagnostic that did not read
/// the query cannot pass.
#[tokio::test]
async fn a_relayed_compile_failure_states_the_place_in_the_submitted_query() {
    let logs = Logs::new();
    const GROUP: &str = "/envelope/diagnostic";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let (start, end) = window();

    // (case, the query string, the symbol the compiler stops on, the line it is on)
    let cases: Vec<(&str, &str, &str, usize)> = vec![
        (
            "an unknown command right after the first stage",
            "fields @message | abc",
            "abc",
            1,
        ),
        (
            "the same unknown command further along",
            "fields @timestamp, @message | limit 10 | abc",
            "abc",
            1,
        ),
        (
            "the same unknown command further along still",
            "fields @timestamp, @message | filter @message like /ERROR/ | limit 10 | abc",
            "abc",
            1,
        ),
        (
            "a longer unknown command",
            "fields @message | frobnicate x",
            "frobnicate",
            1,
        ),
        (
            // One character is one token, so the separator written against the next command is
            // named as the character it is.
            "a separator standing where an operand was owed",
            "fields |sort @timestamp",
            "|",
            1,
        ),
        (
            "an unknown command on the second line of the query string",
            "fields @message\n| shuffle @message",
            "shuffle",
            2,
        ),
    ];

    for (case, query_string, symbol, line) in cases {
        let request = json!({
            "logGroupName": GROUP,
            "startTime": start,
            "endTime": end,
            "queryString": query_string,
        });
        let (status, body, text, request_id) = refused(&logs, case, "StartQuery", &request).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case}: expected the refusal status, got {status} with {text}"
        );
        assert_eq!(
            crate::harness::error_code(&body),
            MALFORMED_QUERY,
            "case {case}: expected {MALFORMED_QUERY}, got {text}"
        );
        // The place, read off the submitted query string: the line the symbol is on, counted
        // from one, and its character position within that line, counted from zero.
        let at = query_string
            .find(symbol)
            .unwrap_or_else(|| panic!("case {case}: the row's query carries {symbol:?}"));
        let before = &query_string[..at];
        assert_eq!(
            before.matches('\n').count() + 1,
            line,
            "case {case}: the row states the line its symbol is on"
        );
        let position = before
            .rsplit_once('\n')
            .map_or(before, |(_, last_line)| last_line)
            .chars()
            .count();
        let said = message(case, &body);
        assert_eq!(
            crate::harness::without_sdk_envelope(case, &said, REFUSAL_STATUS, MALFORMED_QUERY),
            format!("unexpected symbol found {symbol} at line {line} and position {position}"),
            "case {case}: expected the diagnostic for {query_string:?} before the envelope, \
             got {text}"
        );
        // The envelope names the request THIS response reports, and no other: the id is in the
        // clause and nowhere else in the sentence.
        assert!(
            said.ends_with(&expected_clause(MALFORMED_QUERY, &request_id)),
            "case {case}: expected the clause stating the reported id, got {text}"
        );
        // "Details about this error are displayed in a QueryCompileError object": the detail
        // locates the same text, as the character range it occupies in the submitted string.
        let location = &body["queryCompileError"]["location"];
        let span = (
            location["startCharOffset"].as_i64(),
            location["endCharOffset"].as_i64(),
        );
        let first = i64::try_from(query_string[..at].chars().count()).unwrap_or(i64::MAX);
        assert_eq!(
            span,
            (
                Some(first),
                Some(first + i64::try_from(symbol.chars().count()).unwrap_or(i64::MAX))
            ),
            "case {case}: the compile error locates the symbol it stopped on, got {text}"
        );
    }
}
