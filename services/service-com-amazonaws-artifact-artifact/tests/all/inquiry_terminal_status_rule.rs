// SPDX-License-Identifier: CC0-1.0

//! What an inquiry's TERMINAL STATUS reports, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is one rule over every form of submitted content:
//!
//! * an inquiry's terminal `InquiryStatus`/`InquiryStatusMessage` report what became of
//!   the content it was SUBMITTED with. The modelled processing-failure pair is reserved
//!   for a submission that carried nothing to process (an upload with no bytes); a
//!   submission that carried content is processed and reaches `COMPLETED` — the one support
//!   mode `InquirySupportMode` admits is an AI review, which the clock finishes — whatever
//!   handle its units of work turn out to be reportable under;
//! * how many queries the inquiry can REPORT is a bound of the member that reports them
//!   (`QuerySummary$query` is `@required`, `@length(min: 1, max: 2048)`, `@pattern
//!   ^[^<>]*$`), so a unit of work the submission gave no handle this member can carry is
//!   reported as no query — and that changes neither the terminal pair nor the units the
//!   submission did give a reportable handle;
//! * no query of an inquiry this emulator holds ever reports a failure: a query is
//!   processed by reading the submitted content, nothing here does that, and no failure is
//!   synthesised for work nothing ran — so neither the query-level failure values nor the
//!   inquiry-level completed-with-query-errors value is ever reported.
//!
//! The rows differ along what the rule generalises over: the form of the content (a single
//! submitted query versus an uploaded document), how many bytes the upload carries (none,
//! a few, many), how many sections it names (none, one, two), the title it was submitted
//! under (ordinary, absent, only characters the `query` member excludes, a single space,
//! longer than the member admits) and whether `supportMode` is given.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The span after which an inquiry's processing is due to be over, plus slack. The
/// handlers store the due instant rather than sleeping, so only the test waits.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio
/// without its `time` feature.
const PAST_THE_PROCESSING_SPAN: Duration = Duration::from_millis(900);

/// `InquiryStatusMessage`'s values, spelled out rather than imported from the crate so a
/// changed constant fails the suite instead of redefining it
/// (`smithy_query(query="shape",shape="InquiryStatusMessage")`).
const INQUIRY_IN_PROGRESS: &str = "Compliance inquiry processing is in-progress.";
/// See [`INQUIRY_IN_PROGRESS`].
const INQUIRY_COMPLETE: &str = "Compliance inquiry processing is complete.";
/// See [`INQUIRY_IN_PROGRESS`].
const INQUIRY_INTERNAL_ERROR: &str =
    "An internal error occurred while processing the inquiry. Try again at a later time.";
/// See [`INQUIRY_IN_PROGRESS`]. The value no inquiry may report: nothing in this emulator
/// fails a query.
const INQUIRY_COMPLETE_WITH_QUERY_ERRORS: &str = "Compliance inquiry processing is complete. \
     One or more queries encountered errors during processing.";

/// `QueryStatusMessage`'s values
/// (`smithy_query(query="shape",shape="QueryStatusMessage")`).
const QUERY_IN_PROGRESS: &str = "Query processing is in-progress.";
/// See [`QUERY_IN_PROGRESS`].
const QUERY_COMPLETE: &str = "Query processing is complete.";
/// See [`QUERY_IN_PROGRESS`]. The values no query may report: nothing in this emulator
/// fails a query.
const QUERY_INTERNAL_ERROR: &str =
    "An internal error occurred while processing the query. Try again at a later time.";
/// See [`QUERY_INTERNAL_ERROR`].
const QUERY_RESTRICTED_CONTENT: &str = "Query contains restricted or unsupported content.";

/// The longest `query` a `QuerySummary` can report: the member targets
/// `LongStringAttribute`, `@length(min: 1, max: 2048)` with `@pattern ^[^<>]*$`
/// (`smithy_query(query="shape",shape="LongStringAttribute")`).
const QUERY_TEXT_CEILING: usize = 2048;

/// A SigV4 credential scope naming this service, which is what routes the request.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob: the 13 bytes of
/// "questionnaire".
const UPLOAD: &str = "cXVlc3Rpb25uYWlyZQ==";

/// An upload carrying NO bytes — `InquiryFileContent$content` is `@required` and the model
/// bounds its length in no other way
/// (`smithy_query(query="shape",shape="InquiryFileContent")`), so an empty blob is a
/// well-formed request and the one submission with nothing to process.
const EMPTY_UPLOAD: &str = "";

/// A larger document: base64 encodes three bytes per four characters, so repeating one
/// group is a longer upload without an encoder — 192 bytes of "que" repeated.
fn long_upload() -> String {
    "cXVl".repeat(64)
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Bytes) -> Answer {
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", SIGNED)
        .body(body)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Create one inquiry and return the `InquiryId` it minted.
async fn create(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let answer = send(
        router,
        "POST",
        "/v1/compliance-inquiry/create",
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        answer.status, 202,
        "case {case:?}: CreateComplianceInquiry must answer HTTP 202, got {} with {:?}",
        answer.status, answer.body
    );
    let summary = &answer.body["complianceInquirySummary"];
    assert_eq!(
        (summary["status"].as_str(), summary["statusMessage"].as_str()),
        (Some("PROCESSING"), Some(INQUIRY_IN_PROGRESS)),
        "case {case:?}: the create answers a non-terminal inquiry, got {summary:?}"
    );
    summary["id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the create must report an inquiry id, got {:?}", answer.body)
        })
        .to_string()
}

/// The `complianceInquiryDetail` `GetComplianceInquiryMetadata` reports for `id`.
async fn metadata(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let answer = send(
        router,
        "GET",
        &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={id}"),
        Bytes::new(),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetComplianceInquiryMetadata must find the inquiry, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["complianceInquiryDetail"].clone()
}

/// The `queries` `ListComplianceInquiryQueries` reports for `id`.
async fn queries(router: &MultiServiceRouter, case: &str, id: &str) -> Vec<Value> {
    let answer = send(
        router,
        "GET",
        &format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={id}"),
        Bytes::new(),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: ListComplianceInquiryQueries must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["queries"].as_array().cloned().unwrap_or_default()
}

/// A `CreateComplianceInquiry` body for an uploaded document.
fn file_body(title: &str, content: &str, sections: &[&str], support_mode: Option<&str>) -> Value {
    let mut file = json!({ "content": content });
    if !sections.is_empty() {
        file["fileSections"] = json!(sections);
    }
    let mut body = json!({ "name": title, "inquiryContent": { "fileContent": file } });
    if let Some(mode) = support_mode {
        body["supportMode"] = json!(mode);
    }
    body
}

/// One submission, and what the inquiry it creates must report once its processing span is
/// over.
struct Case {
    label: &'static str,
    body: Value,
    /// The `InquiryStatus`/`InquiryStatusMessage` the inquiry rests in.
    terminal: (&'static str, &'static str),
    /// The query texts the inquiry reports, in ascending `queryIdentifier` order.
    reported: Vec<String>,
    /// The `QueryStatus`/`QueryStatusMessage` each reported query rests in.
    query_terminal: (&'static str, &'static str),
}

/// The submissions this suite covers.
fn cases() -> Vec<Case> {
    let completed = ("COMPLETED", QUERY_COMPLETE);
    vec![
        Case {
            label: "a submitted query",
            body: json!({
                "name": "Data residency",
                "inquiryContent": { "query": "Where is customer data stored?" },
            }),
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            reported: vec!["Where is customer data stored?".to_string()],
            query_terminal: completed,
        },
        Case {
            label: "an upload titled for its document",
            body: file_body("Vendor questionnaire", UPLOAD, &[], None),
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            reported: vec!["Vendor questionnaire".to_string()],
            query_terminal: completed,
        },
        Case {
            label: "an upload submitted under no title at all",
            body: file_body("", UPLOAD, &[], None),
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            reported: Vec::new(),
            query_terminal: completed,
        },
        Case {
            label: "an upload titled only with characters the query member excludes",
            body: file_body("<>", UPLOAD, &[], Some("AI_ONLY")),
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            reported: Vec::new(),
            query_terminal: completed,
        },
        Case {
            label: "a larger upload titled only with excluded characters",
            body: file_body("<<<>>>", &long_upload(), &[], None),
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            reported: Vec::new(),
            query_terminal: completed,
        },
        Case {
            label: "an upload titled with a single space, which the member can carry",
            body: file_body(" ", UPLOAD, &[], None),
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            reported: vec![" ".to_string()],
            query_terminal: completed,
        },
        Case {
            label: "an upload under no title that names two sections",
            body: file_body("", UPLOAD, &["Sheet 1", "Sheet 2"], Some("AI_ONLY")),
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            reported: vec!["Sheet 1".to_string(), "Sheet 2".to_string()],
            query_terminal: completed,
        },
        Case {
            label: "a larger upload under no title",
            body: file_body("", &long_upload(), &[], Some("AI_ONLY")),
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            reported: Vec::new(),
            query_terminal: completed,
        },
        Case {
            label: "an upload titled with excluded characters, one section",
            body: file_body("<>", UPLOAD, &["Controls 1-20"], Some("AI_ONLY")),
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            reported: vec!["Controls 1-20".to_string()],
            query_terminal: completed,
        },
        Case {
            label: "an upload whose title is longer than the query member admits",
            body: file_body(&"a".repeat(QUERY_TEXT_CEILING + 101), UPLOAD, &[], None),
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            reported: vec!["a".repeat(QUERY_TEXT_CEILING)],
            query_terminal: completed,
        },
        Case {
            label: "an upload carrying no bytes, so nothing to process",
            body: file_body("Empty workbook", EMPTY_UPLOAD, &["Section A"], None),
            terminal: ("FAILED", INQUIRY_INTERNAL_ERROR),
            reported: Vec::new(),
            query_terminal: completed,
        },
        Case {
            label: "an upload carrying no bytes under no title",
            body: file_body("", EMPTY_UPLOAD, &[], Some("AI_ONLY")),
            terminal: ("FAILED", INQUIRY_INTERNAL_ERROR),
            reported: Vec::new(),
            query_terminal: completed,
        },
    ]
}

#[tokio::test]
async fn an_inquiry_reports_what_became_of_the_content_it_was_submitted_with() {
    for case in cases() {
        let label = case.label;
        // One emulator per case, so the span one case waits out cannot decide what
        // another observes.
        let router = emulator();
        let id = create(&router, label, &case.body).await;

        // Inside the processing span nothing is decided yet, whatever the submission was.
        let pending = metadata(&router, label, &id).await;
        assert_eq!(
            (pending["status"].as_str(), pending["statusMessage"].as_str()),
            (Some("PROCESSING"), Some(INQUIRY_IN_PROGRESS)),
            "case {label:?}: an inquiry read inside its processing span is still \
             processing, got {pending:?}"
        );
        for query in &queries(&router, label, &id).await {
            assert_eq!(
                (query["status"].as_str(), query["statusMessage"].as_str()),
                (Some("PROCESSING"), Some(QUERY_IN_PROGRESS)),
                "case {label:?}: a query read inside the processing span is still \
                 processing, got {query:?}"
            );
        }

        std::thread::sleep(PAST_THE_PROCESSING_SPAN);

        let detail = metadata(&router, label, &id).await;
        assert_eq!(
            (detail["status"].as_str(), detail["statusMessage"].as_str()),
            (Some(case.terminal.0), Some(case.terminal.1)),
            "case {label:?}: expected the terminal pair {:?}, got {detail:?}",
            case.terminal
        );
        assert_ne!(
            detail["statusMessage"].as_str(),
            Some(INQUIRY_COMPLETE_WITH_QUERY_ERRORS),
            "case {label:?}: no query of this inquiry failed, so the query-error value \
             must not be reported, got {detail:?}"
        );

        // The queries the inquiry reports are the units of work it was given a handle
        // this API's `query` member can carry — and nothing about them moves the pair
        // above.
        let listed = queries(&router, label, &id).await;
        let reported: Vec<&str> = listed
            .iter()
            .map(|query| query["query"].as_str().unwrap_or_default())
            .collect();
        let expected: Vec<&str> = case.reported.iter().map(String::as_str).collect();
        assert_eq!(
            reported, expected,
            "case {label:?}: expected the reported queries {expected:?}, got {listed:?}"
        );
        for (position, query) in listed.iter().enumerate() {
            let identifier = i64::try_from(position).expect("a row index fits in i64") + 1;
            assert_eq!(
                query["queryIdentifier"].as_i64(),
                Some(identifier),
                "case {label:?}: queries are reported in ascending sequential \
                 queryIdentifier, expected {identifier}, got {query:?}"
            );
            assert_eq!(
                (query["status"].as_str(), query["statusMessage"].as_str()),
                (Some(case.query_terminal.0), Some(case.query_terminal.1)),
                "case {label:?}: expected the query pair {:?} past the processing span, \
                 got {query:?}",
                case.query_terminal
            );
            assert!(
                !["FAILED"].contains(&query["status"].as_str().unwrap_or_default())
                    && ![QUERY_INTERNAL_ERROR, QUERY_RESTRICTED_CONTENT]
                        .contains(&query["statusMessage"].as_str().unwrap_or_default()),
                "case {label:?}: nothing here processes a query's content, so no query \
                 may report a failure, got {query:?}"
            );
            assert_eq!(
                query["response"].is_null(),
                query["citations"].is_null(),
                "case {label:?}: a response and the citations supporting it are reported \
                 together, got {query:?}"
            );
        }

        // A read changes nothing a later caller sees: the pair above was owed by the
        // clock, not minted by the reader that happened to look first.
        let again = metadata(&router, label, &id).await;
        assert_eq!(
            (again["status"].as_str(), again["statusMessage"].as_str()),
            (Some(case.terminal.0), Some(case.terminal.1)),
            "case {label:?}: a second read must report the same terminal pair, got {again:?}"
        );
        assert_eq!(
            queries(&router, label, &id).await,
            listed,
            "case {label:?}: a second listing must report the same records, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn only_a_submission_with_nothing_to_process_reports_a_processing_failure() {
    for case in cases() {
        let label = case.label;
        let router = emulator();
        let id = create(&router, label, &case.body).await;
        std::thread::sleep(PAST_THE_PROCESSING_SPAN);

        let detail = metadata(&router, label, &id).await;
        let carried_nothing = case.terminal.0 == "FAILED";
        assert_eq!(
            detail["status"].as_str() == Some("FAILED"),
            carried_nothing,
            "case {label:?}: the modelled failure state belongs to a submission with \
             nothing to process alone, got {detail:?}"
        );
        assert_eq!(
            detail["statusMessage"].as_str() == Some(INQUIRY_INTERNAL_ERROR),
            carried_nothing,
            "case {label:?}: the internal-error message belongs to a submission with \
             nothing to process alone, got {detail:?}"
        );
    }
}
