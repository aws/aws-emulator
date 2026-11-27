// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH REQUEST the envelope of a CloudWatch Logs refusal names, and for
//! the refusals that carry one.
//!
//! RULE: EVERY refusal message of this service — one it DECIDES about the request and one it
//! RELAYS from the query compiler — is the sentence for its class followed by the SDK error
//! envelope naming the request THIS RESPONSE answers: ` (Service: AWSLogs; Status Code: <the
//! status answered>; Error Code: <the code answered>; Request ID: <this response's own id>;
//! Proxy: null)`. A compile failure's own text is the SENTENCE that clause follows, and the
//! diagnostic alone travels in the modelled `queryCompileError` detail beside it. The response
//! also reports its request in the header this protocol names one in (`x-amzn-RequestId`), and
//! the id stated in the clause is that same value, so a client correlating the two reads ONE id
//! for one call.
//!
//! The table spans what the rule generalises over: whether the refusal is relayed from the
//! compiler or decided by the service, which fault the compiler reported (a symbol it stopped
//! on, the end of the query string, the snippet syntax a query that has to select its own log
//! groups is read as, a function the language defines no name for, a regular expression that
//! does not compile, and a field the pipeline already carries), and which class the refusal is
//! answered under — including a `MalformedQueryException` the SERVICE decides over the time
//! range rather than over the query text, so what is asserted is the rule and not one recorded
//! refusal.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.StartQuery` through the router that mounts it — so a message no
//! handler actually wrote cannot pass.

use serde_json::{json, Value};

use crate::harness::{now_ms, Logs};

/// The operation under test.
const START: &str = "StartQuery";

/// The status every refusal below is answered with.
const REFUSAL_STATUS: u16 = 400;

/// The error classes the rows are refused under, each declared by the operation that answers it
/// (`smithy_query(query="operation",operation="StartQuery")`).
const MALFORMED_QUERY: &str = "MalformedQueryException";
/// See [`MALFORMED_QUERY`].
const INVALID_PARAMETER: &str = "InvalidParameterException";
/// See [`MALFORMED_QUERY`].
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The fields of the envelope clause, so a sentence can be shown to carry none of them and the
/// modelled detail beside the message to carry none either.
const ENVELOPE_FRAMING: [&str; 5] = [
    "(Service:",
    "Status Code:",
    "Error Code:",
    "Request ID:",
    "Proxy:",
];

/// The header this protocol names the request a response answers in.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// What the sentence in front of the envelope has to be: the whole text where a row can state
/// it, or the text it opens with where the rest of it is the compiler's own rendering of a
/// fault (an expression's compile error, a span) that this suite does not pin.
#[derive(Clone, Copy, Debug)]
enum Sentence {
    /// The sentence, in full.
    Exact(&'static str),
    /// What the sentence opens with.
    Opening(&'static str),
}

/// A window in the unit `StartQuery` documents its range in ("epoch time, the number of seconds
/// since January 1, 1970"), an hour wide and ending now.
fn window() -> (i64, i64) {
    let now_secs = now_ms() / 1_000 + 1;
    (now_secs - 60 * 60, now_secs)
}

/// The envelope the rule requires: the status and the code the response answers with, and the
/// request it names.
fn expected_clause(code: &str, request_id: &str) -> String {
    format!(
        " (Service: AWSLogs; Status Code: {REFUSAL_STATUS}; Error Code: {code}; Request ID: \
         {request_id}; Proxy: null)"
    )
}

/// One refused call: the status, the parsed body, its text and the id the response reports for
/// the request it answers.
async fn refused(logs: &Logs, case: &str, request: &Value) -> (u16, Value, String, String) {
    let (status, headers, payload) = logs.call_raw(START, request).await;
    let text = String::from_utf8(payload.to_vec())
        .unwrap_or_else(|_| panic!("case {case}: the error body is text, got {payload:?}"));
    let body: Value = serde_json::from_str(&text)
        .unwrap_or_else(|_| panic!("case {case}: the error body is JSON, got {text:?}"));
    let reported = headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| panic!("case {case}: the response names the request it answers"))
        .to_string();
    (status, body, text, reported)
}

/// The message a refusal reports.
fn message(case: &str, body: &Value, text: &str) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: a refusal carries a message, got {text}"))
        .to_string()
}

/// RULE: every refusal — relayed from the compiler or decided by the service — states its
/// sentence and then the envelope naming this response's own request.
#[tokio::test]
async fn every_refusal_states_its_sentence_inside_the_envelope_naming_this_response() {
    let logs = Logs::new();
    const GROUP: &str = "/relayed/one";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let (start, end) = window();

    // (case, the request, the class it is refused under, the sentence its envelope follows)
    let cases: Vec<(&str, Value, &str, Sentence)> = vec![
        (
            "a query whose second stage is no command of the language",
            json!({
                "logGroupName": GROUP,
                "startTime": start,
                "endTime": end,
                "queryString": "fields @message | abc",
            }),
            MALFORMED_QUERY,
            // "fields @message | " is 18 characters, so the unknown command opens there.
            Sentence::Exact("unexpected symbol found abc at line 1 and position 18"),
        ),
        (
            "a query that ends where an operand was owed",
            json!({
                "logGroupName": GROUP,
                "startTime": start,
                "endTime": end,
                "queryString": "fields @message | sort",
            }),
            MALFORMED_QUERY,
            // The compiler ran out of input at the end of a 22-character query string.
            Sentence::Exact("unexpected symbol found <EOF> at line 1 and position 22"),
        ),
        (
            // No member names the log groups, so the query string is read as the snippet that
            // has to select them, and the compiler stops on its opening symbol.
            "a query that selects its log groups nowhere",
            json!({
                "startTime": start,
                "endTime": end,
                "queryString": "fields @message",
            }),
            MALFORMED_QUERY,
            Sentence::Exact(
                "Invalid syntax while using query definition snippets: unexpected symbol found \
                 fields at line 1 and position 0",
            ),
        ),
        (
            "a call to a function the language defines no name for",
            json!({
                "logGroupName": GROUP,
                "startTime": start,
                "endTime": end,
                "queryString": "fields @message | stats bogusfn(@message)",
            }),
            MALFORMED_QUERY,
            Sentence::Opening("Unrecognized function name: bogusfn ("),
        ),
        (
            "a parse command whose regular expression does not compile",
            json!({
                "logGroupName": GROUP,
                "startTime": start,
                "endTime": end,
                "queryString": r"fields @message | parse @message /lat=(?<lat>\d+/",
            }),
            MALFORMED_QUERY,
            Sentence::Opening("Compile Error - Invalid regular expression in query: "),
        ),
        (
            "a pipeline that defines a field it already carries",
            json!({
                "logGroupName": GROUP,
                "startTime": start,
                "endTime": end,
                "queryString": "fields @message | parse @message \"lat=*\" as lat | parse \
                                @message \"lon=*\" as lat",
            }),
            MALFORMED_QUERY,
            Sentence::Opening("Ephemeral field is already defined: lat "),
        ),
        (
            // The same error CLASS, decided by the service over the time range rather than
            // relayed from the compiler: the clause it states is the same one.
            "a query window that closes before the log group existed",
            json!({
                "logGroupName": GROUP,
                "startTime": start - 10 * 24 * 60 * 60,
                "endTime": start - 9 * 24 * 60 * 60,
                "queryString": "fields @message",
            }),
            MALFORMED_QUERY,
            Sentence::Opening("Query's end date and time is either before the log groups "),
        ),
        (
            "a query window that runs backwards",
            json!({
                "logGroupName": GROUP,
                "startTime": end,
                "endTime": start,
                "queryString": "fields @message",
            }),
            INVALID_PARAMETER,
            Sentence::Opening("End time cannot be less than Start time"),
        ),
        (
            "a query over a log group this account does not hold",
            json!({
                "logGroupName": "/relayed/absent",
                "startTime": start,
                "endTime": end,
                "queryString": "fields @message",
            }),
            NOT_FOUND,
            Sentence::Opening("Log group '/relayed/absent' does not exist for account ID "),
        ),
    ];

    for (case, request, code, sentence) in cases {
        let (status, body, text, reported) = refused(&logs, case, &request).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case}: expected the refusal status, got {status} with {text}"
        );
        assert_eq!(
            crate::harness::error_code(&body),
            code,
            "case {case}: expected {code}, got {text}"
        );
        let said = message(case, &body, &text);
        let clause = expected_clause(code, &reported);
        assert!(
            said.ends_with(&clause),
            "case {case}: expected the envelope {clause:?} stating the reported id, got {said:?}"
        );
        // ONE envelope, after a sentence that says what is wrong.
        let said_sentence = said.strip_suffix(&clause).unwrap_or_default().to_string();
        assert!(
            !said_sentence.trim().is_empty() && !said_sentence.contains("(Service:"),
            "case {case}: expected one sentence and one envelope, got {said:?}"
        );
        match sentence {
            Sentence::Exact(expected) => assert_eq!(
                said_sentence, expected,
                "case {case}: expected the sentence {expected:?}, got {said_sentence:?}"
            ),
            Sentence::Opening(opening) => assert!(
                said_sentence.starts_with(opening),
                "case {case}: expected a sentence opening {opening:?}, got {said_sentence:?}"
            ),
        }
        // "Details about this error are displayed in a QueryCompileError object": where the
        // refusal carries that detail, the detail is the diagnostic ALONE — the envelope is
        // part of the message the service answers with and of nothing else.
        if let Some(detail) = body["queryCompileError"]["message"].as_str() {
            for framing in ENVELOPE_FRAMING {
                assert!(
                    !detail.contains(framing),
                    "case {case}: expected no {framing:?} framing in the detail, got {detail:?}"
                );
            }
            assert!(
                !detail.contains(&reported),
                "case {case}: the detail names no request, got {detail:?}"
            );
        }
    }
}

/// RULE (the other dimension): two identical calls refused by the COMPILER state the same
/// sentence, while each response's envelope names its OWN request — so the per-call value in a
/// relayed message is the id the response itself reports and nothing else.
#[tokio::test]
async fn a_relayed_refusal_states_one_sentence_and_each_responses_own_request() {
    let logs = Logs::new();
    const GROUP: &str = "/relayed/per-call";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let (start, end) = window();
    let request = json!({
        "logGroupName": GROUP,
        "startTime": start,
        "endTime": end,
        "queryString": "fields @message | frobnicate x",
    });

    let mut sentences: Vec<String> = Vec::new();
    let mut reported_ids: Vec<String> = Vec::new();
    for attempt in 0..3 {
        let case = format!("call {attempt}");
        let (status, body, text, reported) = refused(&logs, &case, &request).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case}: expected the refusal status, got {status} with {text}"
        );
        let said = message(&case, &body, &text);
        let clause = expected_clause(MALFORMED_QUERY, &reported);
        let said_sentence = said
            .strip_suffix(&clause)
            .unwrap_or_else(|| {
                panic!("case {case}: expected the envelope {clause:?}, got {said:?}")
            })
            .to_string();
        sentences.push(said_sentence);
        reported_ids.push(reported);
    }
    let first = sentences.first().expect("the loop ran at least once").clone();
    assert!(
        sentences.iter().all(|said| *said == first),
        "expected one sentence for the same relayed refusal, got {sentences:?}"
    );
    let mut distinct = reported_ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        reported_ids.len(),
        "expected one request id per response, got {reported_ids:?}"
    );
}
