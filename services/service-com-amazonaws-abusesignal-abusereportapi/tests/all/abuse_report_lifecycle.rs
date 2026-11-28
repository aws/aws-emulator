// SPDX-License-Identifier: CC0-1.0

//! The abuse-report lifecycle, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a restJson1
//! request — so what is asserted is what an SDK client sees: the status, the JSON body,
//! and the state the next call observes.
//!
//! The rules under test, each exercised over a SET of inputs rather than one request:
//!
//! * `CreateAbuseReport` mints a `reportId` of the modelled form, PERSISTS the submission,
//!   and `GetAbuseReportStatus` and `ListAbuseReportStatuses` report that stored record —
//!   the same id, correlation id, status, description and timestamps;
//! * a submission repeated under one idempotency token answers with the report the first
//!   one created, inside the window the model documents for that token, and a token reused
//!   with a different payload is the modelled conflict;
//! * a report moves `QUEUED` → `PROCESSING` → `COMPLETED` as the CLOCK makes each step
//!   due, carrying a `reasonCode` only in the terminal status, and nothing moves it after;
//! * a listing reports the stored reports in submission order, filters them by
//!   `correlationId`, cuts them into pages of `maxResults`, and hands back a `nextToken`
//!   only while reports under the same filter remain;
//! * a `nextToken` this listing did not issue names no page;
//! * a report that is not well-formed is refused with the modelled 400, and one that is
//!   well-formed but fails a submission requirement with the modelled 422 carrying that
//!   requirement's published code and JSON Pointer.
//!
//! Nothing here waits: the emulator's clock is STOPPED by the harness, and a case that has
//! to cross a span advances it.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_abusesignal_abusereportapi::ops::types::REASON_CODE_VALUES;
use service_com_amazonaws_abusesignal_abusereportapi::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region the caller's credential scope names. Deliberately one of the Regions the
/// model mentions rather than the first one anything defaults to.
const REGION: &str = "eu-west-2";

/// `ReportId`'s modelled bounds (`@length(20..64)`, `^[a-zA-Z0-9]{20,64}$`).
const REPORT_ID_MIN_CHARS: usize = 20;
/// See [`REPORT_ID_MIN_CHARS`].
const REPORT_ID_MAX_CHARS: usize = 64;

/// `maxResults`' modelled range for `ListAbuseReportStatuses` (1..50).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 50;

/// How close two timestamps the service derived from one stored instant have to be for
/// the assertion to hold. The derivation is exact; the slack is for the precision an
/// epoch-second timestamp loses as a JSON number, and nothing here measures the host.
const TIMESTAMP_SLACK_SECONDS: f64 = 1e-3;

/// An abuse-signal emulator and the one wire path into it.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        // Every case decides its own time: with the clock stopped, no lifecycle step
        // falls due until a case advances it, so a status this file asserts cannot depend
        // on how fast the host ran the case before it.
        service_common::clock::stop(&*engine).expect("the in-memory engine holds a clock");
        Self { router, engine }
    }

    /// Move this emulator's clock on by `span`, which is how a case crosses a step it
    /// does not wait for.
    fn advance(&self, span: Duration) {
        service_common::clock::advance(&*self.engine, span)
            .expect("the in-memory engine holds a clock");
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serializable request body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://abuse.{REGION}.amazonaws.com{path}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260112/{REGION}/abuse/aws4_request, \
                     SignedHeaders=host, Signature=abc"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|error| {
                panic!(
                    "{method} {path} answered {status} with a body that is not JSON ({error}): \
                     {:?}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        (status, parsed)
    }

    async fn create(&self, body: Value) -> (u16, Value) {
        self.call("POST", "/v1/abuse", Some(body)).await
    }

    /// Submit a report that must be accepted, answering its `reportId`.
    async fn create_ok(&self, case: &str, body: Value) -> String {
        let (status, response) = self.create(body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateAbuseReport, got {status} ({response})"
        );
        text(&response, "reportId")
    }

    async fn get(&self, report_id: &str) -> (u16, Value) {
        self.call("GET", &format!("/v1/abuse/{report_id}"), None).await
    }

    async fn get_ok(&self, case: &str, report_id: &str) -> Value {
        let (status, response) = self.get(report_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetAbuseReportStatus, got {status} \
             ({response})"
        );
        response
    }

    async fn list(&self, query: &str) -> (u16, Value) {
        self.call("GET", &format!("/v1/abusestatuses{query}"), None)
            .await
    }

    async fn list_ok(&self, case: &str, query: &str) -> Value {
        let (status, response) = self.list(query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListAbuseReportStatuses, got {status} \
             ({response})"
        );
        response
    }
}

/// A submission body carrying `document` and, where the row names them, a `correlationId`
/// and a `clientToken`.
fn submission(document: &Value, correlation_id: Option<&str>, client_token: Option<&str>) -> Value {
    let mut body = json!({ "report": { "xarf": document } });
    if let Some(value) = correlation_id {
        body["correlationId"] = json!(value);
    }
    if let Some(value) = client_token {
        body["clientToken"] = json!(value);
    }
    body
}

/// The smallest XARF document this service accepts: the two core fields the model calls
/// required ("the required `category` and `type` fields that classify the report (for
/// example, `category` `content` with `type` `phishing`)").
fn core_document(kind: &str) -> Value {
    json!({ "category": "content", "type": kind })
}

/// A string member of a response, or the empty string when it carries none.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A timestamp member of a response, in seconds since the epoch.
fn instant(case: &str, body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} timestamp, got {body}"))
}

/// The error code `restJson1` puts in `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` an error body carries.
fn message(body: &Value) -> String {
    text(body, "message")
}

/// The report ids one page reports, in the order it reports them.
fn page_ids(case: &str, page: &Value) -> Vec<String> {
    page.get("reports")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a reports list, got {page}"))
        .iter()
        .map(|item| text(item, "reportId"))
        .collect()
}

/// Whether `value` is of the form `ReportId` declares.
fn is_report_id(value: &str) -> bool {
    (REPORT_ID_MIN_CHARS..=REPORT_ID_MAX_CHARS).contains(&value.chars().count())
        && value.chars().all(|character| character.is_ascii_alphanumeric())
}

/// The query string of a listing under `filter` with `page_size`, resuming at `token`.
fn listing_query(filter: Option<&str>, page_size: Option<i64>, token: Option<&str>) -> String {
    let mut parameters = Vec::new();
    if let Some(value) = filter {
        parameters.push(format!("correlationId={value}"));
    }
    if let Some(value) = page_size {
        parameters.push(format!("maxResults={value}"));
    }
    if let Some(value) = token {
        parameters.push(format!("nextToken={value}"));
    }
    if parameters.is_empty() {
        String::new()
    } else {
        format!("?{}", parameters.join("&"))
    }
}

/// Every report id a complete walk of the listing reports, following `nextToken` to the
/// end, asserting on the way that no page is larger than asked for, that no report is
/// reported twice, and that only the last page carries no token.
async fn walk(emulator: &Emulator, case: &str, filter: Option<&str>, page_size: Option<i64>) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut token: Option<String> = None;
    // A listing of a bounded store ends; the bound keeps a token that resumes where it
    // was handed out from looping here instead of failing.
    for page_number in 1..=64 {
        let query = listing_query(filter, page_size, token.as_deref());
        let page = emulator.list_ok(case, &query).await;
        let ids = page_ids(case, &page);
        if let Some(size) = page_size {
            assert!(
                i64::try_from(ids.len()).unwrap_or(i64::MAX) <= size,
                "case {case:?}: page {page_number} must hold at most {size} reports, got {}",
                ids.len()
            );
        }
        for id in ids {
            assert!(
                seen.insert(id.clone()),
                "case {case:?}: page {page_number} reported {id} a second time, after {walked:?}"
            );
            walked.push(id);
        }
        match page.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => return walked,
        }
    }
    panic!("case {case:?}: the listing never ended, after {walked:?}");
}

/// A create, a read and a listing report ONE stored record — the same id, correlation id,
/// status, description and timestamps — whatever optional members the submission carried
/// and whatever the shape of its XARF document.
#[tokio::test]
async fn a_created_report_is_read_back_and_listed_from_the_stored_record() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: whether the submission
    // carries a correlationId, whether it carries a clientToken, and the shape of the
    // XARF document (flat, deeply nested, carrying a list, numbers and booleans, and the
    // core fields in another order).
    let rows: [(&str, Option<&str>, Option<&str>, Value); 5] = [
        (
            "neither optional member",
            None,
            None,
            core_document("phishing"),
        ),
        (
            "a correlation id alone",
            Some("case-1"),
            None,
            core_document("spam"),
        ),
        (
            "a client token alone",
            None,
            Some("tok-ddos"),
            core_document("ddos"),
        ),
        (
            "both, and a nested document",
            Some("case_2-X"),
            Some("tok-malware"),
            json!({
                "category": "content",
                "type": "malware",
                "reporter": { "org": "example", "contact": { "email": "abuse@example.com" } },
                "evidence": [{ "description": "a sample" }, { "description": "another" }],
                "observed_count": 3,
                "confirmed": true,
                "note": null
            }),
        ),
        (
            "the core fields in another order",
            Some("CASE3"),
            None,
            json!({ "type": "copyright", "category": "content" }),
        ),
    ];

    let mut submitted: Vec<(String, Option<&str>)> = Vec::new();
    for (case, correlation_id, client_token, document) in rows {
        let (status, created) = emulator
            .create(submission(&document, correlation_id, client_token))
            .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateAbuseReport, got {status} ({created})"
        );
        let report_id = text(&created, "reportId");
        assert!(
            is_report_id(&report_id),
            "case {case:?}: expected a reportId of 20..64 alphanumeric characters, got \
             {report_id:?}"
        );
        assert_eq!(
            created.get("correlationId").and_then(Value::as_str),
            correlation_id,
            "case {case:?}: expected the submitted correlationId echoed back, got {created}"
        );
        assert!(
            submitted.iter().all(|(seen, _)| seen != &report_id),
            "case {case:?}: expected a reportId no other submission got, got {report_id:?} \
             after {submitted:?}"
        );

        let read = emulator.get_ok(case, &report_id).await;
        assert_eq!(
            text(&read, "reportId"),
            report_id,
            "case {case:?}: expected the created reportId back, got {read}"
        );
        assert_eq!(
            read.get("correlationId").and_then(Value::as_str),
            correlation_id,
            "case {case:?}: expected the stored correlationId, got {read}"
        );
        assert_eq!(
            text(&read, "status"),
            state::STATUS_QUEUED,
            "case {case:?}: expected a freshly submitted report to be queued, got {read}"
        );
        assert_eq!(
            text(&read, "description"),
            state::QUEUED_DESCRIPTION,
            "case {case:?}: expected the queued description, got {read}"
        );
        assert_eq!(
            read.get("reasonCode"),
            None,
            "case {case:?}: expected no reasonCode before the terminal status, got {read}"
        );
        let submitted_at = instant(case, &read, "submittedAt");
        assert!(
            (instant(case, &read, "lastUpdatedAt") - submitted_at).abs() < TIMESTAMP_SLACK_SECONDS,
            "case {case:?}: expected lastUpdatedAt at the submission instant {submitted_at}, got \
             {read}"
        );
        submitted.push((report_id, correlation_id));
    }

    // The listing reports every stored report, in submission order, with the same values
    // the read answered with.
    let page = emulator.list_ok("every report", "").await;
    assert_eq!(
        page_ids("every report", &page),
        submitted
            .iter()
            .map(|(report_id, _)| report_id.clone())
            .collect::<Vec<String>>(),
        "expected every report in submission order, got {page}"
    );
    assert_eq!(
        page.get("nextToken"),
        None,
        "expected no nextToken on a page that ends the listing, got {page}"
    );
    let items = page["reports"].as_array().expect("a reports list").clone();
    for ((report_id, correlation_id), item) in submitted.iter().zip(items) {
        assert_eq!(
            item.get("correlationId").and_then(Value::as_str),
            *correlation_id,
            "case {report_id}: expected the stored correlationId in the listing, got {item}"
        );
        assert_eq!(
            text(&item, "status"),
            state::STATUS_QUEUED,
            "case {report_id}: expected the stored status in the listing, got {item}"
        );
        assert_eq!(
            text(&item, "description"),
            state::QUEUED_DESCRIPTION,
            "case {report_id}: expected the stored description in the listing, got {item}"
        );
        assert_eq!(
            item.get("reasonCode"),
            None,
            "case {report_id}: expected no reasonCode before the terminal status, got {item}"
        );
    }
}

/// A submission repeated under one `clientToken` answers with the report the first one
/// created; the same token with a DIFFERENT payload is the modelled conflict; and once the
/// documented window has closed the token answers nothing.
#[tokio::test]
async fn a_submission_repeated_under_one_client_token_answers_with_the_stored_report() {
    let emulator = Emulator::new();
    let document = json!({
        "category": "content",
        "type": "phishing",
        "source_identifier": "192.0.2.10",
        "reporter": { "org": "example" }
    });
    let token = Some("tok-1");
    let correlation = Some("case-1");
    let first = emulator
        .create_ok(
            "the first submission",
            submission(&document, correlation, token),
        )
        .await;

    // The SAME payload: byte-identical, and the same members in another order — a
    // document's member order is no part of its value.
    let reordered = json!({
        "reporter": { "org": "example" },
        "source_identifier": "192.0.2.10",
        "type": "phishing",
        "category": "content"
    });
    for (case, body) in [
        (
            "the identical submission",
            submission(&document, correlation, token),
        ),
        (
            "the same members in another order",
            submission(&reordered, correlation, token),
        ),
    ] {
        let again = emulator.create_ok(case, body).await;
        assert_eq!(
            again, first,
            "case {case:?}: expected the reportId the first submission created ({first})"
        );
    }

    // A DIFFERENT payload under the same token: rows differing in how they differ — a
    // changed value, an added member, a removed member, and another correlationId.
    let rows: [(&str, Value); 4] = [
        (
            "a changed value",
            submission(
                &json!({
                    "category": "content",
                    "type": "spam",
                    "source_identifier": "192.0.2.10",
                    "reporter": { "org": "example" }
                }),
                correlation,
                token,
            ),
        ),
        (
            "an added member",
            submission(
                &json!({
                    "category": "content",
                    "type": "phishing",
                    "source_identifier": "192.0.2.10",
                    "reporter": { "org": "example" },
                    "description": "and a word about it"
                }),
                correlation,
                token,
            ),
        ),
        (
            "a removed member",
            submission(
                &json!({
                    "category": "content",
                    "type": "phishing",
                    "reporter": { "org": "example" }
                }),
                correlation,
                token,
            ),
        ),
        (
            "another correlation id",
            submission(&document, Some("case-2"), token),
        ),
    ];
    for (case, body) in rows {
        let (status, refused) = emulator.create(body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409 for a token reused with a different payload, got \
             {status} ({refused})"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: expected the modelled conflict, got {refused}"
        );
        assert!(
            message(&refused).contains("idempotency token"),
            "case {case:?}: expected a message about the reused idempotency token, got {refused}"
        );
    }

    // Only the first submission created anything.
    assert_eq!(
        page_ids(
            "after the conflicts",
            &emulator.list_ok("after the conflicts", "").await
        ),
        vec![first.clone()],
        "expected the refused submissions to have stored nothing"
    );

    // Once the documented 24-hour window has closed the token answers nothing, so the
    // same submission creates a second report.
    emulator.advance(state::CLIENT_TOKEN_WINDOW);
    let after = emulator
        .create_ok(
            "after the clientToken window",
            submission(&document, correlation, token),
        )
        .await;
    assert_ne!(
        after, first,
        "case after the clientToken window: expected a new report once the token's window \
         closed"
    );
    assert_eq!(
        page_ids(
            "after the window",
            &emulator.list_ok("after the window", "").await
        ),
        vec![first, after],
        "expected exactly the two reports the two live windows created"
    );
}

/// A submission that carries NO `clientToken` is answered under the token the service
/// derives for it, which the model scopes to five minutes.
#[tokio::test]
async fn a_submission_with_no_client_token_is_scoped_to_the_generated_window() {
    let emulator = Emulator::new();
    let document = json!({ "category": "messaging", "type": "spam" });
    let first = emulator
        .create_ok("the first submission", submission(&document, None, None))
        .await;

    let again = emulator
        .create_ok(
            "the same submission inside the generated window",
            submission(&document, None, None),
        )
        .await;
    assert_eq!(
        again, first,
        "case the same submission inside the generated window: expected the report the first \
         one created ({first})"
    );

    // A different payload is a different token, not a conflict.
    let other = emulator
        .create_ok(
            "another submission inside the generated window",
            submission(&core_document("ddos"), None, None),
        )
        .await;
    assert_ne!(
        other, first,
        "case another submission inside the generated window: expected a report of its own"
    );

    emulator.advance(state::GENERATED_TOKEN_WINDOW);
    let after = emulator
        .create_ok(
            "the same submission after the generated window",
            submission(&document, None, None),
        )
        .await;
    assert_ne!(
        after, first,
        "case the same submission after the generated window: expected a new report once the \
         derived token's window closed"
    );
}

/// A `reportId` no submission minted is the modelled 404 — for every well-formed id,
/// while an id that WAS minted is served.
#[tokio::test]
async fn a_report_id_no_submission_minted_is_not_found() {
    let emulator = Emulator::new();
    let minted = emulator
        .create_ok("a stored report", submission(&core_document("spam"), None, None))
        .await;

    // The rows differ along the form of a well-formed id: the shortest and the longest
    // `ReportId` admits, digits only, letters only, and a mixed-case value.
    let rows: [(&str, String); 5] = [
        ("the shortest id the pattern admits", "A".repeat(REPORT_ID_MIN_CHARS)),
        ("the longest id the pattern admits", "b".repeat(REPORT_ID_MAX_CHARS)),
        ("digits only", "0".repeat(32)),
        ("letters only", "abcdefghijklmnopqrstuvwxyz".to_string()),
        ("a mixed-case value", "Ab12Cd34Ef56Gh78Ij90".to_string()),
    ];
    for (case, report_id) in rows {
        assert_ne!(
            report_id, minted,
            "case {case:?}: the row must name an id no submission minted"
        );
        let (status, refused) = emulator.get(&report_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for an id no submission minted, got {status} \
             ({refused})"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled not-found, got {refused}"
        );
        assert_eq!(
            text(&refused, "resourceId"),
            report_id,
            "case {case:?}: expected the id that was looked up in resourceId, got {refused}"
        );
    }

    // The same operation serves the id that was minted, so the refusal above is about the
    // store rather than about the shape of an id.
    assert_eq!(
        text(&emulator.get_ok("a stored report", &minted).await, "reportId"),
        minted,
        "expected the stored report to be served"
    );
}

/// A report moves through the reporter-facing lifecycle as the CLOCK makes each step due,
/// carries a `reasonCode` only in the terminal status, and stops there.
#[tokio::test]
async fn the_lifecycle_steps_fall_due_on_the_clock_and_stop_at_the_terminal_status() {
    let emulator = Emulator::new();
    let report_id = emulator
        .create_ok(
            "a submitted report",
            submission(&core_document("phishing"), Some("case-1"), None),
        )
        .await;
    let queued = emulator.get_ok("queued", &report_id).await;
    let submitted_at = instant("queued", &queued, "submittedAt");
    assert_eq!(
        text(&queued, "status"),
        state::STATUS_QUEUED,
        "case queued: expected a freshly submitted report to be queued, got {queued}"
    );

    // A step that has not fallen due has not happened: one tick short of the span, the
    // report is still where it was.
    emulator.advance(state::PROGRESSION_SPAN - Duration::from_millis(1));
    let still_queued = emulator.get_ok("a tick short of the first step", &report_id).await;
    assert_eq!(
        text(&still_queued, "status"),
        state::STATUS_QUEUED,
        "case a tick short of the first step: expected the report still queued, got \
         {still_queued}"
    );

    // Each step in turn, each reported with its own description and with `lastUpdatedAt`
    // at the instant the step fell due rather than when it was observed.
    emulator.advance(Duration::from_millis(1));
    let processing = emulator.get_ok("processing", &report_id).await;
    assert_eq!(
        text(&processing, "status"),
        state::STATUS_PROCESSING,
        "case processing: expected the first step materialised, got {processing}"
    );
    assert_eq!(
        text(&processing, "description"),
        state::PROCESSING_DESCRIPTION,
        "case processing: expected the processing description, got {processing}"
    );
    assert_eq!(
        processing.get("reasonCode"),
        None,
        "case processing: expected no reasonCode before the terminal status, got {processing}"
    );
    let expected_processing_at = submitted_at + state::PROGRESSION_SPAN.as_secs_f64();
    assert!(
        (instant("processing", &processing, "lastUpdatedAt") - expected_processing_at).abs()
            < TIMESTAMP_SLACK_SECONDS,
        "case processing: expected lastUpdatedAt at {expected_processing_at}, got {processing}"
    );
    assert!(
        (instant("processing", &processing, "submittedAt") - submitted_at).abs()
            < TIMESTAMP_SLACK_SECONDS,
        "case processing: expected submittedAt to stay at {submitted_at}, got {processing}"
    );

    emulator.advance(state::PROGRESSION_SPAN);
    let completed = emulator.get_ok("completed", &report_id).await;
    assert_eq!(
        text(&completed, "status"),
        state::STATUS_COMPLETED,
        "case completed: expected the terminal status, got {completed}"
    );
    assert_eq!(
        text(&completed, "description"),
        state::COMPLETED_DESCRIPTION,
        "case completed: expected the completed description, got {completed}"
    );
    let reason_code = text(&completed, "reasonCode");
    assert!(
        REASON_CODE_VALUES.contains(&reason_code.as_str()),
        "case completed: expected a reasonCode the model publishes ({REASON_CODE_VALUES:?}), got \
         {reason_code:?}"
    );
    let expected_completed_at = submitted_at + state::PROGRESSION_SPAN.as_secs_f64() * 2.0;
    assert!(
        (instant("completed", &completed, "lastUpdatedAt") - expected_completed_at).abs()
            < TIMESTAMP_SLACK_SECONDS,
        "case completed: expected lastUpdatedAt at {expected_completed_at}, got {completed}"
    );

    // The terminal status is terminal, and a read reports the same record however often
    // it is read and however much further the clock runs.
    for (case, span) in [
        ("read again at the same instant", Duration::ZERO),
        ("read again much later", state::PROGRESSION_SPAN * 100),
    ] {
        emulator.advance(span);
        let again = emulator.get_ok(case, &report_id).await;
        assert_eq!(
            again, completed,
            "case {case:?}: expected the terminal record unchanged, got {again}"
        );
    }

    // The listing reports the same stored record as the read.
    let page = emulator.list_ok("the completed report", "").await;
    let item = page["reports"]
        .as_array()
        .and_then(|items| items.first().cloned())
        .unwrap_or_else(|| panic!("expected one listed report, got {page}"));
    assert_eq!(
        (
            text(&item, "reportId"),
            text(&item, "status"),
            text(&item, "description"),
            text(&item, "reasonCode")
        ),
        (
            report_id,
            state::STATUS_COMPLETED.to_string(),
            state::COMPLETED_DESCRIPTION.to_string(),
            reason_code
        ),
        "expected the listing to report the record the read reported, got {item}"
    );
}

/// An empty store lists no reports, which is a page rather than an error.
#[tokio::test]
async fn an_empty_store_lists_no_reports() {
    let emulator = Emulator::new();
    for (case, query) in [
        ("no parameters", String::new()),
        ("a filter", listing_query(Some("case-1"), None, None)),
        (
            "a page size",
            listing_query(None, Some(MAX_RESULTS_MIN), None),
        ),
    ] {
        let page = emulator.list_ok(case, &query).await;
        assert_eq!(
            page_ids(case, &page),
            Vec::<String>::new(),
            "case {case:?}: expected an empty list, got {page}"
        );
        assert_eq!(
            page.get("nextToken"),
            None,
            "case {case:?}: expected no nextToken, got {page}"
        );
    }
}

/// The listing filters by `correlationId` and, for any page size, walks exactly the
/// reports under that filter — each once, in submission order.
#[tokio::test]
async fn the_listing_filters_by_correlation_id_and_walks_every_report_exactly_once() {
    let emulator = Emulator::new();

    // Seven submissions interleaved across two correlation ids and none, so neither the
    // filter nor the order can be satisfied by a store that groups them.
    let filters = [
        Some("case-1"),
        Some("case-2"),
        None,
        Some("case-1"),
        None,
        Some("case-2"),
        Some("case-1"),
    ];
    let mut submitted: Vec<(String, Option<&str>)> = Vec::new();
    for (position, correlation_id) in filters.into_iter().enumerate() {
        let document = json!({
            "category": "content",
            "type": "phishing",
            "description": format!("submission {position}")
        });
        let case = format!("submission {position}");
        let report_id = emulator
            .create_ok(&case, submission(&document, correlation_id, None))
            .await;
        submitted.push((report_id, correlation_id));
    }
    let under = |filter: Option<&str>| -> Vec<String> {
        submitted
            .iter()
            .filter(|(_, correlation_id)| filter.is_none() || *correlation_id == filter)
            .map(|(report_id, _)| report_id.clone())
            .collect()
    };

    // Rows: no filter, each filter that matches, and one that matches nothing. Each is
    // walked at every page size that matters — the modelled bounds, sizes that divide the
    // run and sizes that do not, and no page size at all.
    for (case, filter) in [
        ("no filter", None),
        ("the first filter", Some("case-1")),
        ("the second filter", Some("case-2")),
        ("a filter nothing matches", Some("case-3")),
    ] {
        let expected = if filter == Some("case-3") {
            Vec::new()
        } else {
            under(filter)
        };
        for page_size in [
            None,
            Some(MAX_RESULTS_MIN),
            Some(2),
            Some(3),
            Some(6),
            Some(MAX_RESULTS_MAX),
        ] {
            let case = format!("{case} at maxResults={page_size:?}");
            let walked = walk(&emulator, &case, filter, page_size).await;
            assert_eq!(
                walked, expected,
                "case {case:?}: expected exactly the reports under the filter, in submission \
                 order"
            );
        }
    }

    // A `maxResults` outside the modelled range is refused rather than clamped.
    for (case, value) in [
        ("below the modelled range", MAX_RESULTS_MIN - 1),
        ("past the modelled range", MAX_RESULTS_MAX + 1),
    ] {
        let (status, refused) = emulator.list(&listing_query(None, Some(value), None)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for maxResults={value}, got {status} ({refused})"
        );
    }
}

/// A `nextToken` this listing did not issue names no page: it is refused with the modelled
/// 400, while a token the listing DID issue is served.
#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let mut submitted = Vec::new();
    for position in 0..3 {
        let document = json!({
            "category": "content",
            "type": "phishing",
            "description": format!("submission {position}")
        });
        submitted.push(
            emulator
                .create_ok(
                    &format!("submission {position}"),
                    submission(&document, Some("case-1"), None),
                )
                .await,
        );
    }

    // A token this listing issued is served, and resumes where the page it ended.
    let first_page = emulator
        .list_ok("the first page", &listing_query(None, Some(1), None))
        .await;
    let issued = first_page
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a nextToken on a page of one of three, got {first_page}"))
        .to_string();
    let resumed = emulator
        .list_ok(
            "the issued token",
            &listing_query(None, Some(1), Some(&issued)),
        )
        .await;
    assert_eq!(
        page_ids("the issued token", &resumed),
        vec![submitted[1].clone()],
        "case the issued token: expected the page after the first, got {resumed}"
    );

    // A token issued by ANOTHER store, for the same position: the cursor is authenticated
    // under the issuing store's own secret, so it names no position here.
    let other = Emulator::new();
    for position in 0..3 {
        let document = json!({
            "category": "content",
            "type": "phishing",
            "description": format!("submission {position}")
        });
        other
            .create_ok(
                &format!("other submission {position}"),
                submission(&document, Some("case-1"), None),
            )
            .await;
    }
    let other_page = other
        .list_ok("another store's first page", &listing_query(None, Some(1), None))
        .await;
    let other_token = other_page
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("another store's page of one of three carries a nextToken")
        .to_string();

    // A token issued for the UNFILTERED listing, replayed under a filter: a cursor names
    // a position in one sequence, and that is not this one.
    let rows: [(&str, String); 6] = [
        ("not hex at all", "not-a-token".to_string()),
        (
            "hex of a report id",
            ::hex::encode(submitted[0].as_bytes()),
        ),
        (
            "hex of an authenticator the caller made up",
            ::hex::encode([vec![0_u8; 32], submitted[0].as_bytes().to_vec()].concat()),
        ),
        (
            "an issued token with its last byte dropped",
            issued[..issued.len() - 2].to_string(),
        ),
        (
            "an issued token with a byte changed",
            format!(
                "{}{}",
                &issued[..issued.len() - 1],
                if issued.ends_with('0') { "1" } else { "0" }
            ),
        ),
        ("a token another store issued", other_token),
    ];
    for (case, token) in rows {
        let (status, refused) = emulator
            .list(&listing_query(None, Some(1), Some(&token)))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a token this listing did not issue, got \
             {status} ({refused})"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected the modelled validation refusal, got {refused}"
        );
        assert_eq!(
            refused.get("reports"),
            None,
            "case {case:?}: expected a refusal to carry no page, got {refused}"
        );
    }

    // The same token under a different filter names no position in THAT sequence.
    let (status, refused) = emulator
        .list(&listing_query(Some("case-1"), Some(1), Some(&issued)))
        .await;
    assert_eq!(
        status, 400,
        "case a token issued for another filter: expected HTTP 400, got {status} ({refused})"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case a token issued for another filter: expected the modelled validation refusal, got \
         {refused}"
    );
}

/// A report that does not satisfy the XARF schema is refused as NOT WELL-FORMED — the
/// modelled 400, naming the member — and nothing is stored for it.
#[tokio::test]
async fn a_report_that_is_not_well_formed_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();

    // The rows differ along how the document fails the schema: it is not an object at
    // all, it omits one of the two core fields the model calls required, it carries one
    // as a value of another type, and it carries one as the empty string.
    let rows: [(&str, Value, &str); 7] = [
        ("a string where the report goes", json!("just some text"), ""),
        ("a list where the report goes", json!([1, 2, 3]), ""),
        (
            "no category",
            json!({ "type": "phishing" }),
            "/category",
        ),
        (
            "no type",
            json!({ "category": "content" }),
            "/type",
        ),
        (
            "a category that is not a string",
            json!({ "category": 7, "type": "phishing" }),
            "/category",
        ),
        (
            "an empty type",
            json!({ "category": "content", "type": "" }),
            "/type",
        ),
        (
            "a type of whitespace alone",
            json!({ "category": "content", "type": "   " }),
            "/type",
        ),
    ];
    for (case, document, path) in rows {
        let (status, refused) = emulator.create(submission(&document, None, None)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a report that is not well-formed, got {status} \
             ({refused})"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected the modelled validation refusal, got {refused}"
        );
        let paths: Vec<String> = refused
            .get("fieldList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected a fieldList, got {refused}"))
            .iter()
            .map(|field| text(field, "path"))
            .collect();
        assert!(
            paths.contains(&path.to_string()),
            "case {case:?}: expected fieldList to name {path:?}, got {paths:?}"
        );
    }

    // A refused submission stored nothing.
    let page = emulator.list_ok("after the refusals", "").await;
    assert_eq!(
        page_ids("after the refusals", &page),
        Vec::<String>::new(),
        "expected a refused submission to have stored nothing, got {page}"
    );
}

/// A report that IS well-formed but does not meet a submission requirement is refused with
/// the modelled 422, carrying that requirement's published code and JSON Pointer — and a
/// report that meets every one of them is accepted.
#[tokio::test]
async fn a_report_that_fails_a_submission_requirement_is_refused_with_its_published_code() {
    let emulator = Emulator::new();

    // A 253-character domain name: the maximum the model accepts for a
    // `source_identifier` ("the DNS hostname limit"), and one character more.
    let at_the_limit = format!("{}.com", "a".repeat(state::SOURCE_IDENTIFIER_MAX_CHARS - 4));
    let past_the_limit = format!("{}.com", "a".repeat(state::SOURCE_IDENTIFIER_MAX_CHARS - 3));
    // The signature bytes a sniffer recognises each format by.
    let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
    let gif = *b"GIF89a\0\0\0\0\0\0";

    // One row per requirement the model publishes, each differing in the member it is
    // about and in how that member breaks the requirement.
    let rows: [(&str, Value, &str, &str); 8] = [
        (
            "a source identifier one character past the accepted length",
            json!({ "category": "content", "type": "phishing", "source_identifier": past_the_limit }),
            state::SOURCE_IDENTIFIER_TOO_LONG,
            "/source_identifier",
        ),
        (
            "a source identifier carrying a template marker",
            json!({ "category": "content", "type": "phishing", "source_identifier": "{{host}}.example.com" }),
            state::SOURCE_IDENTIFIER_FORBIDDEN_CHARACTERS,
            "/source_identifier",
        ),
        (
            "a source identifier carrying a wildcard",
            json!({ "category": "content", "type": "phishing", "source_identifier": "*.example.com" }),
            state::SOURCE_IDENTIFIER_FORBIDDEN_CHARACTERS,
            "/source_identifier",
        ),
        (
            "a source identifier carrying a command marker",
            json!({ "category": "content", "type": "phishing", "source_identifier": "`id`.example.com" }),
            state::SOURCE_IDENTIFIER_FORBIDDEN_CHARACTERS,
            "/source_identifier",
        ),
        (
            "a source identifier that is no address at all",
            json!({ "category": "content", "type": "phishing", "source_identifier": "the host next door" }),
            state::SOURCE_IDENTIFIER_MALFORMED,
            "/source_identifier",
        ),
        (
            "a timestamp that is no date-time",
            json!({ "category": "content", "type": "phishing", "timestamp": "yesterday" }),
            state::TIMESTAMP_UNPARSEABLE,
            "/timestamp",
        ),
        (
            "a timestamp past the clock-skew tolerance",
            json!({
                "category": "content",
                "type": "phishing",
                "timestamp": rendered(SystemTime::now() + state::CLOCK_SKEW_TOLERANCE * 2)
            }),
            state::TIMESTAMP_IN_FUTURE,
            "/timestamp",
        ),
        (
            "a timestamp before the submission lookback window",
            json!({
                "category": "content",
                "type": "phishing",
                "timestamp": rendered(SystemTime::now() - state::SUBMISSION_LOOKBACK_WINDOW - Duration::from_secs(86_400))
            }),
            state::TIMESTAMP_TOO_OLD,
            "/timestamp",
        ),
    ];
    for (case, document, code, path) in rows {
        let (status, refused) = emulator.create(submission(&document, None, None)).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected HTTP 422 for an unmet submission requirement, got {status} \
             ({refused})"
        );
        assert_eq!(
            error_code(&refused),
            "SubmissionRequirementException",
            "case {case:?}: expected the modelled submission-requirement refusal, got {refused}"
        );
        let unmet = refused
            .get("unmetRequirements")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected unmetRequirements, got {refused}"))
            .clone();
        assert_eq!(
            unmet
                .iter()
                .map(|clause| (text(clause, "code"), text(clause, "path")))
                .collect::<Vec<(String, String)>>(),
            vec![(code.to_string(), path.to_string())],
            "case {case:?}: expected the published code and pointer of the requirement, got \
             {refused}"
        );
    }

    // An evidence item whose decoded payload is not what it declares, with the pointer
    // naming the item.
    for (case, declared, payload, position) in [
        ("the first item", "text/plain", png.to_vec(), 0_usize),
        ("the second item", "image/png", gif.to_vec(), 1_usize),
    ] {
        let document = json!({
            "category": "content",
            "type": "malware",
            "evidence": [
                {
                    "content_type": if position == 0 { declared } else { "image/png" },
                    "payload": ::aws_smithy_types::base64::encode(if position == 0 { payload.clone() } else { png.to_vec() })
                },
                {
                    "content_type": if position == 1 { declared } else { "image/gif" },
                    "payload": ::aws_smithy_types::base64::encode(if position == 1 { payload.clone() } else { gif.to_vec() })
                }
            ]
        });
        let (status, refused) = emulator.create(submission(&document, None, None)).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected HTTP 422 for an evidence payload that is not what it \
             declares, got {status} ({refused})"
        );
        let unmet = refused
            .get("unmetRequirements")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected unmetRequirements, got {refused}"))
            .clone();
        assert_eq!(
            unmet
                .iter()
                .map(|clause| (text(clause, "code"), text(clause, "path")))
                .collect::<Vec<(String, String)>>(),
            vec![(
                state::EVIDENCE_CONTENT_TYPE_MISMATCH.to_string(),
                format!("/evidence/{position}")
            )],
            "case {case:?}: expected the mismatch named at the item it is about, got {refused}"
        );
    }

    // Nothing was stored for any of them.
    assert_eq!(
        page_ids(
            "after the refusals",
            &emulator.list_ok("after the refusals", "").await
        ),
        Vec::<String>::new(),
        "expected a refused submission to have stored nothing"
    );

    // The same members INSIDE every requirement are accepted: each form the model admits
    // for a source identifier, the length limit itself, a timestamp on either side of
    // now, and an evidence payload that is what it declares.
    let accepted: [(&str, Value); 9] = [
        (
            "an IPv4 source identifier",
            json!({ "category": "content", "type": "phishing", "source_identifier": "192.0.2.10" }),
        ),
        (
            "an IPv6 source identifier",
            json!({ "category": "content", "type": "phishing", "source_identifier": "2001:db8::1" }),
        ),
        (
            "a URL source identifier",
            json!({ "category": "content", "type": "phishing", "source_identifier": "https://example.com/a/phishing/page" }),
        ),
        (
            "a domain-name source identifier",
            json!({ "category": "content", "type": "phishing", "source_identifier": "mail.example.com" }),
        ),
        (
            "a source identifier at the accepted length",
            json!({ "category": "content", "type": "phishing", "source_identifier": at_the_limit }),
        ),
        (
            "a timestamp inside the clock-skew tolerance",
            json!({
                "category": "content",
                "type": "phishing",
                "timestamp": rendered(SystemTime::now() + state::CLOCK_SKEW_TOLERANCE / 2)
            }),
        ),
        (
            "a timestamp inside the submission lookback window",
            json!({
                "category": "content",
                "type": "phishing",
                "timestamp": rendered(SystemTime::now() - state::SUBMISSION_LOOKBACK_WINDOW + Duration::from_secs(86_400))
            }),
        ),
        (
            "an evidence payload that is what it declares",
            json!({
                "category": "content",
                "type": "malware",
                "evidence": [{ "content_type": "image/png", "payload": ::aws_smithy_types::base64::encode(png) }]
            }),
        ),
        (
            "an evidence payload no sniffer has a signature for",
            json!({
                "category": "content",
                "type": "malware",
                "evidence": [{ "content_type": "text/plain", "payload": ::aws_smithy_types::base64::encode(b"a plain note") }]
            }),
        ),
    ];
    let mut stored = Vec::new();
    for (case, document) in accepted {
        stored.push(emulator.create_ok(case, submission(&document, None, None)).await);
    }
    assert_eq!(
        page_ids(
            "the accepted reports",
            &emulator.list_ok("the accepted reports", "").await
        ),
        stored,
        "expected every accepted submission stored, in submission order"
    );
}

/// `at` as the RFC 3339 date-time a XARF `timestamp` carries.
fn rendered(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant renders as an RFC 3339 date-time")
}

/// The store holds the reports it is bounded to, releasing the OLDEST submissions past
/// that bound — and what it released is gone from the read and from the listing alike,
/// never reported as present by one and absent from the other.
#[tokio::test]
async fn the_store_releases_the_oldest_submissions_past_its_bound() {
    let emulator = Emulator::new();
    let past_the_bound = 2;
    let mut submitted: Vec<String> = Vec::new();
    for position in 0..(state::MAX_STORED_REPORTS + past_the_bound) {
        let document = json!({
            "category": "content",
            "type": "phishing",
            "description": format!("submission {position}")
        });
        submitted.push(
            emulator
                .create_ok(
                    &format!("submission {position}"),
                    submission(&document, None, None),
                )
                .await,
        );
    }

    let walked = walk(&emulator, "the bounded store", None, Some(MAX_RESULTS_MAX)).await;
    assert_eq!(
        walked,
        submitted[past_the_bound..].to_vec(),
        "expected the listing to hold the {} most recent submissions, in submission order",
        state::MAX_STORED_REPORTS
    );

    // What the store released is not found, and what it holds is served — the two
    // operations agree about every report either way.
    for (case, report_id, expected) in [
        ("a released submission", submitted[0].clone(), 404),
        ("the next released submission", submitted[1].clone(), 404),
        ("the oldest submission held", submitted[past_the_bound].clone(), 200),
        ("the newest submission", submitted[submitted.len() - 1].clone(), 200),
    ] {
        let (status, body) = emulator.get(&report_id).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected} for {report_id}, got {status} ({body})"
        );
    }
}
