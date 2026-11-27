// SPDX-License-Identifier: CC0-1.0

//! A compliance inquiry's QUERIES and the state they put it in, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is one rule over every form of submitted content:
//!
//! * an inquiry owns ONE QUERY PER UNIT OF WORK its content submits — the query a text
//!   submission carries, and one per section of an uploaded document
//!   (`InquiryFileContent$fileSections`, "List of file sections/sheets to process"), with
//!   the document as a whole when the request names no section — so
//!   `ListComplianceInquiryQueries` reports them in ascending `queryIdentifier` and
//!   `PutComplianceInquiryFeedback` can name each one;
//! * every one of those queries carries the REVIEW the inquiry's support mode asks for,
//!   and `InquirySupportMode` admits exactly one mode — `AI_ONLY`, which "[d]efaults" for
//!   a request naming none (`smithy_query(query="shape",shape="InquirySupportMode")`) — so
//!   every query is `ReviewType` `AI` and reaches `COMPLETED` with "Compliance inquiry
//!   processing is complete.";
//! * the modelled internal-error state is reserved for a submission that carried NOTHING
//!   to process (an upload with no bytes); a submission that carried content reaches a
//!   non-failure terminal status even when no unit of work of it could be reported inside
//!   the modelled bounds of `QuerySummary$query`.
//!
//! A query reports an answer only once its processing is over, and only when the published
//! report catalog holds a document bearing on the handle that unit of work was submitted
//! under: the `response` and the `citations` supporting it are always reported together, and
//! a unit of work the catalog has nothing on reports neither.
//!
//! The rows differ along what the rule generalises over: the form of the content, how many
//! sections an upload names (none, one, several, and `FileSectionList`'s published
//! maximum), the section labels themselves, how many bytes the upload carries, whether
//! `supportMode` is given, and whether the handle a unit of work
//! is reported under fits the modelled bounds of `QuerySummary$query`.

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

/// `InquiryStatusMessage`'s values this suite expects, spelled out rather than imported
/// from the crate so a changed constant fails the suite instead of redefining it
/// (`smithy_query(query="shape",shape="InquiryStatusMessage")`).
const INQUIRY_COMPLETE: &str = "Compliance inquiry processing is complete.";
/// See [`INQUIRY_COMPLETE`].
const INQUIRY_INTERNAL_ERROR: &str =
    "An internal error occurred while processing the inquiry. Try again at a later time.";

/// `QueryStatusMessage`'s value this suite expects
/// (`smithy_query(query="shape",shape="QueryStatusMessage")`).
const QUERY_COMPLETE: &str = "Query processing is complete.";

/// `FileSectionList`'s published maximum length
/// (`smithy_query(query="shape",shape="FileSectionList")`: `@length(min: 0, max: 30)`).
const FILE_SECTIONS_CEILING: usize = 30;

/// The longest `query` a `QuerySummary` can report: the member targets
/// `LongStringAttribute`, `@length(min: 1, max: 2048)` with `@pattern ^[^<>]*$`
/// (`smithy_query(query="shape",shape="LongStringAttribute")`).
const QUERY_TEXT_CEILING: usize = 2048;

/// A SigV4 credential scope naming this service, which is what routes the request.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

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
    answer.body["complianceInquirySummary"]["id"]
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
    answer.body["queries"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the listing must report a queries list, got {:?}",
                answer.body
            )
        })
}

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob: the 13 bytes of
/// "questionnaire".
const UPLOAD: &str = "cXVlc3Rpb25uYWlyZQ==";

/// A second, distinct document: the 14 bytes of "questionnaire2".
const OTHER_UPLOAD: &str = "cXVlc3Rpb25uYWlyZTI=";

/// An upload carrying NO bytes — `InquiryFileContent$content` is `@required` and the
/// model bounds its length in no other way (`smithy_query(query="shape",
/// shape="InquiryFileContent")`), so an empty blob is a well-formed request.
const EMPTY_UPLOAD: &str = "";

/// A larger document: base64 encodes three bytes per four characters, so repeating one
/// group is a longer upload without an encoder — 192 bytes of "que" repeated.
fn long_upload() -> String {
    "cXVl".repeat(64)
}

/// A `CreateComplianceInquiry` body for an uploaded document.
fn file_body(title: &str, content: &str, sections: &[String], support_mode: Option<&str>) -> Value {
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

/// A `CreateComplianceInquiry` body for a single submitted query.
fn text_body(title: &str, query: &str, support_mode: Option<&str>) -> Value {
    let mut body = json!({ "name": title, "inquiryContent": { "query": query } });
    if let Some(mode) = support_mode {
        body["supportMode"] = json!(mode);
    }
    body
}

/// One submission, and what the inquiry it creates must report.
struct Case {
    label: &'static str,
    body: Value,
    /// The query texts the inquiry must own, in ascending `queryIdentifier` order.
    owned: Vec<String>,
    /// The `reviewType` every one of those queries carries.
    review_type: &'static str,
    /// The `InquiryStatus`/`InquiryStatusMessage` the inquiry rests in once its
    /// processing span is over.
    terminal: (&'static str, &'static str),
    /// The `QueryStatus`/`QueryStatusMessage` each of its queries rests in then, or
    /// `None` for a submission that owns no query, where there is no pair to assert.
    query_terminal: Option<(&'static str, &'static str)>,
}

/// A row for an uploaded document.
fn upload(
    label: &'static str,
    title: &'static str,
    content: &str,
    sections: &[&str],
    support_mode: Option<&'static str>,
) -> Case {
    let sections: Vec<String> = sections.iter().map(|value| (*value).to_string()).collect();
    // One query per unit of work the upload submits: the sections it names, or the
    // document as a whole under the submitted title when it names none.
    let owned = if content.is_empty() {
        Vec::new()
    } else if sections.is_empty() {
        vec![title.to_string()]
    } else {
        sections.clone()
    };
    let query_terminal = if owned.is_empty() {
        None
    } else {
        Some(("COMPLETED", QUERY_COMPLETE))
    };
    Case {
        label,
        body: file_body(title, content, &sections, support_mode),
        owned,
        review_type: "AI",
        terminal: if content.is_empty() {
            ("FAILED", INQUIRY_INTERNAL_ERROR)
        } else {
            ("COMPLETED", INQUIRY_COMPLETE)
        },
        query_terminal,
    }
}

/// The submissions this suite covers.
fn cases() -> Vec<Case> {
    // `FileSectionList`'s published maximum, built here so the bound is exercised rather
    // than described. Its member is `ShortStringAttribute`, `@pattern
    // ^[a-zA-Z0-9_\-\s]*$`, so the labels carry only characters it admits.
    let many: Vec<String> = (1..=FILE_SECTIONS_CEILING)
        .map(|index| format!("Sheet {index}"))
        .collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();

    vec![
        upload(
            "an upload naming no section, so the document is the one unit",
            "Vendor questionnaire",
            UPLOAD,
            &[],
            None,
        ),
        upload(
            "an upload naming no section, AI_ONLY, a larger document",
            "Annual audit workbook",
            &long_upload(),
            &[],
            Some("AI_ONLY"),
        ),
        upload(
            "an upload naming one section",
            "Customer DDQ",
            OTHER_UPLOAD,
            &["Sheet 1"],
            Some("AI_ONLY"),
        ),
        upload(
            "an upload naming two sections",
            "Annual audit workbook",
            UPLOAD,
            &["Section A", "Section B"],
            Some("AI_ONLY"),
        ),
        upload(
            "an upload naming one section, a larger document",
            "Security addendum",
            &long_upload(),
            &["Controls 1-20"],
            Some("AI_ONLY"),
        ),
        upload(
            "an upload naming the most sections the list admits",
            "Workbook of sheets",
            UPLOAD,
            &many,
            Some("AI_ONLY"),
        ),
        upload(
            "an upload carrying no bytes, so nothing to process",
            "Empty workbook",
            EMPTY_UPLOAD,
            &["Section A"],
            None,
        ),
        upload(
            "an upload carrying no bytes and naming no section",
            "Empty questionnaire",
            EMPTY_UPLOAD,
            &[],
            Some("AI_ONLY"),
        ),
        // The rows for a handle the modelled `query` shape cannot report verbatim: it is
        // reported inside that shape's bounds, a handle nothing is left of reports no
        // query at all, and reporting no query is not a processing failure — the inquiry
        // still reaches the terminal status its submitted content selects.
        Case {
            label: "an upload whose title carries characters the query shape excludes",
            body: file_body("Workbook <2026> edition", UPLOAD, &[], None),
            owned: vec!["Workbook 2026 edition".to_string()],
            review_type: "AI",
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            query_terminal: Some(("COMPLETED", QUERY_COMPLETE)),
        },
        Case {
            label: "an upload whose title is longer than the query shape admits",
            body: file_body(&"a".repeat(QUERY_TEXT_CEILING + 952), UPLOAD, &[], None),
            owned: vec!["a".repeat(QUERY_TEXT_CEILING)],
            review_type: "AI",
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            query_terminal: Some(("COMPLETED", QUERY_COMPLETE)),
        },
        Case {
            label: "an upload whose title leaves no reportable handle",
            body: file_body("<>", UPLOAD, &[], None),
            owned: Vec::new(),
            review_type: "AI",
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            query_terminal: None,
        },
        Case {
            label: "an upload submitted under no title at all",
            body: file_body("", UPLOAD, &[], None),
            owned: Vec::new(),
            review_type: "AI",
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            query_terminal: None,
        },
        Case {
            label: "a larger upload whose title leaves no reportable handle",
            body: file_body("<>", &long_upload(), &[], Some("AI_ONLY")),
            owned: Vec::new(),
            review_type: "AI",
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            query_terminal: None,
        },
        Case {
            label: "an upload under no title that names its sections",
            body: file_body(
                "",
                UPLOAD,
                &["Sheet 1".to_string(), "Sheet 2".to_string()],
                Some("AI_ONLY"),
            ),
            owned: vec!["Sheet 1".to_string(), "Sheet 2".to_string()],
            review_type: "AI",
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            query_terminal: Some(("COMPLETED", QUERY_COMPLETE)),
        },
        // The contrast rows: a text submission owns the one query it carries, and it is
        // AI-reviewed like every other submission — the model admits one support mode (see
        // `compliance_inquiry_support_mode.rs`).
        Case {
            label: "a submitted query",
            body: text_body("Data residency", "Where is customer data stored?", None),
            owned: vec!["Where is customer data stored?".to_string()],
            review_type: "AI",
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            query_terminal: Some(("COMPLETED", QUERY_COMPLETE)),
        },
        Case {
            label: "a submitted query under AI_ONLY",
            body: text_body(
                "Subprocessors",
                "Which subprocessors handle customer content?",
                Some("AI_ONLY"),
            ),
            owned: vec!["Which subprocessors handle customer content?".to_string()],
            review_type: "AI",
            terminal: ("COMPLETED", INQUIRY_COMPLETE),
            query_terminal: Some(("COMPLETED", QUERY_COMPLETE)),
        },
    ]
}

#[tokio::test]
async fn an_inquiry_owns_one_query_per_unit_of_work_its_content_submitted() {
    for case in cases() {
        let label = case.label;
        // One emulator per case, so the span one case waits out cannot decide what
        // another observes.
        let router = emulator();
        let id = create(&router, label, &case.body).await;

        // Inside the processing span every query is still non-terminal, whatever review
        // it carries: the create answered `PROCESSING` and the clock has not run out.
        let pending = queries(&router, label, &id).await;
        assert_eq!(
            pending.len(),
            case.owned.len(),
            "case {label:?}: expected {} queries, got {pending:?}",
            case.owned.len()
        );
        for query in &pending {
            assert_eq!(
                (
                    query["status"].as_str(),
                    query["statusMessage"].as_str()
                ),
                (Some("PROCESSING"), Some("Query processing is in-progress.")),
                "case {label:?}: a query read inside the processing span is still \
                 processing, got {query:?}"
            );
        }

        std::thread::sleep(PAST_THE_PROCESSING_SPAN);

        let listed = queries(&router, label, &id).await;
        assert_eq!(
            listed.len(),
            case.owned.len(),
            "case {label:?}: expected {} queries, got {listed:?}",
            case.owned.len()
        );
        for (position, (query, expected_text)) in listed.iter().zip(&case.owned).enumerate() {
            let identifier = i64::try_from(position).expect("a row index fits in i64") + 1;
            let query_terminal = case
                .query_terminal
                .expect("a case that owns a query states the pair its queries rest in");
            assert_eq!(
                query["queryIdentifier"].as_i64(),
                Some(identifier),
                "case {label:?}: queries are reported in ascending sequential \
                 queryIdentifier, expected {identifier}, got {query:?}"
            );
            assert_eq!(
                query["query"].as_str(),
                Some(expected_text.as_str()),
                "case {label:?}: expected the unit of work {expected_text:?} at \
                 identifier {identifier}, got {query:?}"
            );
            assert_eq!(
                query["reviewType"].as_str(),
                Some(case.review_type),
                "case {label:?}: expected reviewType {:?}, got {query:?}",
                case.review_type
            );
            assert_eq!(
                (
                    query["status"].as_str(),
                    query["statusMessage"].as_str()
                ),
                (Some(query_terminal.0), Some(query_terminal.1)),
                "case {label:?}: expected the pair {query_terminal:?} past the processing \
                 span, got {query:?}",
            );
            // The answer travels as one, and a section label the catalog holds nothing
            // bearing on is answered with neither member.
            assert_eq!(
                query["response"].is_null(),
                query["citations"].is_null(),
                "case {label:?}: a response and the citations supporting it are reported \
                 together, got {query:?}"
            );
        }

        // The inquiry's own state is derived from those queries, and the metadata read
        // answers from the same stored record.
        let detail = metadata(&router, label, &id).await;
        assert_eq!(
            (
                detail["status"].as_str(),
                detail["statusMessage"].as_str()
            ),
            (Some(case.terminal.0), Some(case.terminal.1)),
            "case {label:?}: expected the terminal pair {:?}, got {detail:?}",
            case.terminal
        );

        // A read changes nothing a later caller sees.
        let again = queries(&router, label, &id).await;
        assert_eq!(
            again, listed,
            "case {label:?}: a second listing must report the same records, got {again:?} \
             against {listed:?}"
        );
    }
}

#[tokio::test]
async fn every_query_an_inquiry_owns_can_be_named_in_feedback() {
    for case in cases() {
        let label = case.label;
        let router = emulator();
        let id = create(&router, label, &case.body).await;
        std::thread::sleep(PAST_THE_PROCESSING_SPAN);

        for (position, _) in case.owned.iter().enumerate() {
            let identifier = i32::try_from(position).expect("a row index fits in i32") + 1;
            let answer = send(
                &router,
                "PUT",
                "/v1/compliance-inquiry/putFeedback",
                Bytes::from(
                    json!({
                        "complianceInquiryId": id,
                        "queryIdentifier": identifier,
                        // A question-level submission is feedback on a VERSION of that
                        // query's response, so it names the revision it is about.
                        "responseRevisionId": 1,
                        "rating": "THUMBS_UP",
                    })
                    .to_string(),
                ),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {label:?}: feedback naming the inquiry's own query {identifier} must \
                 be recorded, got {} with {:?}",
                answer.status, answer.body
            );
        }

        // One past the queries the inquiry owns names none of them, whatever the
        // submission was, so it is refused rather than recorded.
        let beyond = i32::try_from(case.owned.len()).expect("a row count fits in i32") + 1;
        let refused = send(
            &router,
            "PUT",
            "/v1/compliance-inquiry/putFeedback",
            Bytes::from(
                json!({
                    "complianceInquiryId": id,
                    "queryIdentifier": beyond,
                    "rating": "THUMBS_UP",
                })
                .to_string(),
            ),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: identifier {beyond} names no query of this inquiry, so it \
             must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.body["__type"].as_str(),
            Some("ValidationException"),
            "case {label:?}: expected the modelled ValidationException, got {:?}",
            refused.body
        );
    }
}
