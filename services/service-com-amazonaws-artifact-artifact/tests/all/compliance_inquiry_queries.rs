// SPDX-License-Identifier: CC0-1.0

//! `ListComplianceInquiryQueries`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones this operation states:
//!
//! * it REPORTS the queries the inquiry's own content created: as many records as that
//!   content carries, each one the `QuerySummary` projection of the stored query, in
//!   ascending `queryIdentifier` ("[s]equential identifier of the query within the
//!   inquiry"), and nothing is reported for a query nothing created;
//! * a query's `status`/`statusMessage` move with the CLOCK, not with any call: a listing
//!   inside the inquiry's processing span reports the non-terminal pair, a listing after it
//!   reports the terminal pair, and every later listing reports that same record — so a
//!   read never changes what a later caller sees. The answer a query holds is released by
//!   that same transition: while its processing is in progress it reports neither a
//!   `response` nor a `citation`, and the two are always reported together;
//! * the page is bounded by `maxResults` (`@range(min: 1, max: 300)`) and resumed by a
//!   `nextToken` this service issued for THIS inquiry — an OPAQUE cursor, not the
//!   `<inquiryId>:<identifier>` pair it resumes at; a token it did not issue is refused
//!   with the service's one fixed invalid-token `ValidationException`;
//! * an inquiry id this account holds no inquiry for is refused by the gateway in front of
//!   this service (HTTP 401 `UnauthorizedException`), which does not confirm the
//!   resource's absence.
//!
//! The rows differ along what those rules generalise over: the FORM of the submitted
//! content (a single query versus an uploaded document, processed section by section), the
//! query text, whether `supportMode` is given and which value, the requested page size at
//! each published bound and past it, and the shape of the refused token.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::state::{
    inquiry_queries_token, service_state, PageTokenKey,
};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The values `QueryStatus` declares, spelled out here rather than imported from the crate
/// so a status outside the enum fails this suite
/// (`smithy_query(query="shape",shape="QueryStatus")`).
const QUERY_STATUSES: &[&str] = &["PROCESSING", "COMPLETED", "FAILED"];

/// The values `QueryStatusMessage` declares — it is an ENUM, so a `statusMessage` AWS
/// never emits fails this suite
/// (`smithy_query(query="shape",shape="QueryStatusMessage")`).
const QUERY_STATUS_MESSAGES: &[&str] = &[
    "Query processing is complete.",
    "Query processing is in-progress.",
    "An internal error occurred while processing the query. Try again at a later time.",
    "Query is pending human review.",
    "Query contains restricted or unsupported content.",
];

/// The values `ReviewType` declares (`smithy_query(query="shape",shape="ReviewType")`).
const REVIEW_TYPES: &[&str] = &["HUMAN", "AI"];

/// `QueryStatusMessage`'s value a query carries while the inquiry is still processing.
const QUERY_IN_PROGRESS: &str = "Query processing is in-progress.";

/// `QueryStatus`'s value a query ends in, and the message that goes with it: the query
/// text came from the request itself, so its processing is over at the inquiry's due
/// instant — which is also when the query starts reporting the answer it holds.
const QUERY_TERMINAL: (&str, &str) = ("COMPLETED", "Query processing is complete.");

/// The span after which an inquiry's processing is due to be over, plus slack. The handler
/// stores the due instant rather than sleeping, so only the test waits.
///
/// Must stay above `crate::state::INQUIRY_PROCESSING_SPAN` (500 ms at the time of
/// writing): if that span is ever raised, raise this with it.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio without
/// its `time` feature.
const PAST_THE_PROCESSING_SPAN: Duration = Duration::from_millis(900);

/// The fixed text this service answers for a token it did not issue. Spelled out here
/// rather than imported from the crate so a changed constant fails this suite instead of
/// redefining what it asserts.
const INVALID_TOKEN: &str = "The provided token is invalid.";

/// `MaxResultsAttribute`'s published bounds (`@range(min: 1, max: 300)`).
const MAX_RESULTS_FLOOR: i32 = 1;
/// See [`MAX_RESULTS_FLOOR`].
const MAX_RESULTS_CEILING: i32 = 300;

/// A SigV4 credential scope naming this service, which is what routes the request.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob.
const FILE_CONTENT: &str = "cXVlc3Rpb25uYWlyZQ==";

/// One emulator over `engine`, so a test can reach the same store the handler reads.
fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// The secret this emulator's store seals its own pagination cursors under: what a case
/// needing an AUTHENTIC cursor mints one with, through the very codec the handler mints
/// from. A caller holds no such secret, which is why a cursor it assembles is refused.
fn page_token_key(engine: &engine::InMemoryEngine) -> PageTokenKey {
    service_state(engine)
        .expect("the in-memory engine carries this service's store")
        .lock()
        .page_token_key()
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `message` the error body carries, or the empty string when it carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The `queries` a listing reported.
    fn queries(&self) -> &[Value] {
        self.body["queries"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
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
async fn create(router: &MultiServiceRouter, body: &Value) -> String {
    let answer = send(
        router,
        "POST",
        "/v1/compliance-inquiry/create",
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        answer.status, 202,
        "the create must be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["complianceInquirySummary"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the create must report an inquiry id, got {:?}", answer.body))
        .to_string()
}

/// `ListComplianceInquiryQueries` with whatever query string `extra` adds.
async fn list_queries(router: &MultiServiceRouter, inquiry_id: &str, extra: &str) -> Answer {
    let uri =
        format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={inquiry_id}{extra}");
    send(router, "GET", &uri, Bytes::new()).await
}

/// A `CreateComplianceInquiry` body for a single submitted query.
fn text_body(name: &str, query: &str, support_mode: Option<&str>) -> Value {
    body(name, json!({ "query": query }), support_mode)
}

/// A `CreateComplianceInquiry` body for an uploaded document of `content` bytes.
fn file_body(
    name: &str,
    content: &str,
    sections: &[&str],
    support_mode: Option<&str>,
) -> Value {
    let mut file = json!({ "content": content });
    if !sections.is_empty() {
        file["fileSections"] = json!(sections);
    }
    body(name, json!({ "fileContent": file }), support_mode)
}

fn body(name: &str, inquiry_content: Value, support_mode: Option<&str>) -> Value {
    let mut body = json!({ "name": name, "inquiryContent": inquiry_content });
    if let Some(mode) = support_mode {
        body["supportMode"] = json!(mode);
    }
    body
}

/// One submission, and the queries the inquiry it creates must report.
struct Case {
    label: &'static str,
    body: Value,
    /// The query texts the content carries, in `queryIdentifier` order: the submitted
    /// query for a text submission, and one per unit of work an upload is processed in —
    /// the sections it names, or the document as a whole under the submitted title.
    /// EMPTY for an upload carrying no bytes, which submits nothing to process.
    queries: Vec<&'static str>,
    /// The `reviewType` each reported query must carry, if any.
    review_type: Option<&'static str>,
    /// The `status`/`statusMessage` pair each of this case's queries rests in once the
    /// inquiry's processing span is over.
    terminal_pair: (&'static str, &'static str),
}

/// The submissions this suite covers. The rows differ along the form of the content, the
/// query text, how many sections a document names, whether the upload carries any bytes
/// at all, and whether `supportMode` is given.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a query, no supportMode",
            body: text_body("Data residency", "Where is customer data stored?", None),
            queries: vec!["Where is customer data stored?"],
            // Every query of every inquiry is AI-reviewed: `InquirySupportMode` admits one
            // mode, `AI_ONLY` (see `compliance_inquiry_support_mode.rs`).
            review_type: Some("AI"),
            terminal_pair: QUERY_TERMINAL,
        },
        Case {
            label: "a query, AI_ONLY",
            body: text_body(
                "Encryption at rest",
                "Is customer data encrypted at rest?",
                Some("AI_ONLY"),
            ),
            queries: vec!["Is customer data encrypted at rest?"],
            review_type: Some("AI"),
            terminal_pair: QUERY_TERMINAL,
        },
        Case {
            label: "a query, AI_ONLY, other text",
            body: text_body(
                "Subprocessors",
                "Which subprocessors handle customer content?",
                Some("AI_ONLY"),
            ),
            queries: vec!["Which subprocessors handle customer content?"],
            review_type: Some("AI"),
            terminal_pair: QUERY_TERMINAL,
        },
        Case {
            label: "a long query with punctuation",
            body: text_body(
                "Retention",
                "How long is audit log data retained, and where (region / account)?",
                Some("AI_ONLY"),
            ),
            queries: vec!["How long is audit log data retained, and where (region / account)?"],
            review_type: Some("AI"),
            terminal_pair: QUERY_TERMINAL,
        },
        Case {
            label: "an uploaded document, no sections",
            body: file_body("Vendor questionnaire", FILE_CONTENT, &[], None),
            queries: vec!["Vendor questionnaire"],
            review_type: Some("AI"),
            terminal_pair: QUERY_TERMINAL,
        },
        Case {
            label: "an uploaded document, two sections, AI_ONLY",
            body: file_body(
                "Annual audit workbook",
                FILE_CONTENT,
                &["Section A", "Section B"],
                Some("AI_ONLY"),
            ),
            queries: vec!["Section A", "Section B"],
            review_type: Some("AI"),
            terminal_pair: QUERY_TERMINAL,
        },
        Case {
            label: "an uploaded document, three sections, AI_ONLY",
            body: file_body(
                "Customer DDQ",
                FILE_CONTENT,
                &["Sheet 1", "Sheet 2", "Sheet 3"],
                Some("AI_ONLY"),
            ),
            queries: vec!["Sheet 1", "Sheet 2", "Sheet 3"],
            review_type: Some("AI"),
            terminal_pair: QUERY_TERMINAL,
        },
        Case {
            label: "an upload carrying no bytes, so nothing to process",
            body: file_body("Empty workbook", "", &["Section A"], None),
            queries: vec![],
            review_type: None,
            terminal_pair: QUERY_TERMINAL,
        },
    ]
}

/// Assert one reported `QuerySummary` against the submission it came from.
fn assert_summary(
    case: &str,
    position: usize,
    summary: &Value,
    query_text: &str,
    terminal_pair: (&str, &str),
) {
    let expected_identifier = i64::try_from(position).expect("a test row index fits in i64") + 1;
    assert_eq!(
        summary["queryIdentifier"].as_i64(),
        Some(expected_identifier),
        "case {case:?}: queries are reported in ascending sequential queryIdentifier, \
         expected {expected_identifier}, got {:?}",
        summary["queryIdentifier"]
    );
    assert_eq!(
        summary["query"].as_str(),
        Some(query_text),
        "case {case:?}: expected the submitted query text {query_text:?}, got {:?}",
        summary["query"]
    );
    let status = summary["status"].as_str().unwrap_or_default();
    let message = summary["statusMessage"].as_str().unwrap_or_default();
    assert!(
        QUERY_STATUSES.contains(&status),
        "case {case:?}: status must be one of QueryStatus's values, got {status:?}"
    );
    assert!(
        QUERY_STATUS_MESSAGES.contains(&message),
        "case {case:?}: statusMessage must be one of QueryStatusMessage's values, \
         got {message:?}"
    );
    // Read inside the inquiry's processing span or after it, the pair is one of exactly
    // two.
    let legal = [("PROCESSING", QUERY_IN_PROGRESS), terminal_pair];
    assert!(
        legal.contains(&(status, message)),
        "case {case:?}: expected one of {legal:?}, got {:?}",
        (status, message)
    );
    // The answer travels as one: the citations ARE what the response is made of, so
    // neither member is ever reported without the other, and a query still being
    // processed reports neither.
    let response = summary["response"].as_str();
    let citations = summary["citations"].as_array();
    assert_eq!(
        response.is_some(),
        citations.is_some(),
        "case {case:?}: a response and the citations supporting it are reported together, \
         got response {:?} and citations {:?}",
        summary["response"],
        summary["citations"]
    );
    assert!(
        status != "PROCESSING" || response.is_none(),
        "case {case:?}: a query whose processing is still in progress reports no answer, \
         got {:?}",
        summary["response"]
    );
    if let (Some(response), Some(citations)) = (response, citations) {
        assert!(
            !citations.is_empty(),
            "case {case:?}: a reported answer cites the sources it was drawn from, got {:?}",
            summary["citations"]
        );
        for citation in citations {
            let source_label = citation["sourceLabel"].as_str().unwrap_or_default();
            let source_content = citation["sourceContent"].as_str().unwrap_or_default();
            let source_link = citation["sourceLink"].as_str().unwrap_or_default();
            assert!(
                response.contains(source_label) && response.contains(source_content),
                "case {case:?}: the response is the cited passages, so it must carry \
                 {source_label:?} and its passage, got {response:?}"
            );
            assert!(
                source_link.starts_with("https://"),
                "case {case:?}: a citation links to where the source is published, got \
                 {source_link:?}"
            );
        }
        assert!(
            response.chars().count() <= 2048 && !response.contains(['<', '>']),
            "case {case:?}: the response must satisfy LongStringAttribute, got {response:?}"
        );
    }
    assert!(
        summary["updatedResponseVersions"].is_null(),
        "case {case:?}: a response version records an edit to a response, and there is none, \
         got {:?}",
        summary["updatedResponseVersions"]
    );
    assert!(
        !summary["createdAt"].is_null(),
        "case {case:?}: the summary must report when the query was created, got {:?}",
        summary["createdAt"]
    );
}

#[tokio::test]
async fn a_listing_reports_the_queries_the_submitted_content_created() {
    let router = emulator();

    for case in cases() {
        let label = case.label;
        let id = create(&router, &case.body).await;
        let listed = list_queries(&router, &id, "").await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: listing an inquiry's queries must succeed, got {} with {:?}",
            listed.status, listed.body
        );
        assert!(
            listed.body["queries"].is_array(),
            "case {label:?}: the listing must report a queries list, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.queries().len(),
            case.queries.len(),
            "case {label:?}: expected {} queries, got {:?}",
            case.queries.len(),
            listed.body["queries"]
        );
        assert!(
            listed.body["nextToken"].is_null(),
            "case {label:?}: one page holds every query, so no nextToken is owed, got {:?}",
            listed.body["nextToken"]
        );
        for (position, (summary, text)) in listed.queries().iter().zip(&case.queries).enumerate() {
            assert_summary(label, position, summary, text, case.terminal_pair);
            let review = summary["reviewType"].as_str();
            assert_eq!(
                review,
                case.review_type,
                "case {label:?}: expected reviewType {:?}, got {review:?}",
                case.review_type
            );
            if let Some(review) = review {
                assert!(
                    REVIEW_TYPES.contains(&review),
                    "case {label:?}: reviewType must be one of ReviewType's values, \
                     got {review:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_querys_status_moves_with_the_clock_and_a_listing_changes_nothing() {
    let router = emulator();
    let id = create(
        &router,
        &text_body("Access control", "Who can read the audit trail?", Some("AI_ONLY")),
    )
    .await;

    // Inside the processing span: the non-terminal pair the create reported.
    let pending = list_queries(&router, &id, "").await;
    assert_eq!(
        pending
            .queries()
            .first()
            .map(|q| (q["status"].as_str(), q["statusMessage"].as_str())),
        Some((Some("PROCESSING"), Some(QUERY_IN_PROGRESS))),
        "case \"inside the processing span\": the query is still processing, got {:?}",
        pending.body
    );

    std::thread::sleep(PAST_THE_PROCESSING_SPAN);

    let settled = list_queries(&router, &id, "").await;
    assert_eq!(
        settled
            .queries()
            .first()
            .map(|q| (q["status"].as_str(), q["statusMessage"].as_str())),
        Some((Some(QUERY_TERMINAL.0), Some(QUERY_TERMINAL.1))),
        "case \"past the processing span\": the clock owed the query a terminal status, \
         got {:?}",
        settled.body
    );

    // A read must not change what a later caller sees: the next two listings — one of
    // them through another operation's view of the same record — report what this one did.
    let again = list_queries(&router, &id, "").await;
    assert_eq!(
        again.body, settled.body,
        "case \"a repeated listing\": expected {:?}, got {:?}",
        settled.body, again.body
    );
    let metadata = send(
        &router,
        "GET",
        &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={id}"),
        Bytes::new(),
    )
    .await;
    assert_eq!(
        metadata.body["complianceInquiryDetail"]["status"].as_str(),
        Some("COMPLETED"),
        "case \"the inquiry behind the queries\": a listing must leave the inquiry in the \
         status the clock owed it, got {:?}",
        metadata.body
    );
}

#[tokio::test]
async fn a_page_is_bounded_by_max_results_and_resumed_by_a_token_this_service_issued() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let key = page_token_key(&engine);
    let query = "Which controls cover key rotation?";
    let id = create(&router, &text_body("Key rotation", query, None)).await;

    // `maxResults` at each published bound of `MaxResultsAttribute` (@range(min: 1,
    // max: 300)) serves the whole listing: the inquiry's content carries one query, and
    // nothing remains after it, so no `nextToken` is owed at either bound.
    for max_results in [MAX_RESULTS_FLOOR, MAX_RESULTS_CEILING] {
        let case = format!("maxResults={max_results}");
        let listed = list_queries(&router, &id, &format!("&maxResults={max_results}")).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: a page size inside the modelled range must be served, got {} \
             with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.queries().len(),
            1,
            "case {case:?}: expected the inquiry's one query, got {:?}",
            listed.body["queries"]
        );
        assert!(
            listed.body["nextToken"].is_null(),
            "case {case:?}: nothing remains after the page, so no nextToken is owed, \
             got {:?}",
            listed.body["nextToken"]
        );
    }

    // One past each bound is refused by the modelled constraint, as a 400
    // `ValidationException` — not served as a clamped page.
    for max_results in [MAX_RESULTS_FLOOR - 1, MAX_RESULTS_CEILING + 1] {
        let case = format!("maxResults={max_results}");
        let refused = list_queries(&router, &id, &format!("&maxResults={max_results}")).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a page size outside the modelled range must be refused, got {} \
             with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
    }

    // A token this service issued for THIS inquiry is a resume point: one naming the
    // first position serves the query again, and one naming a position past the last
    // serves an empty page rather than failing. The token is the OPAQUE cursor this
    // operation cuts, so it is built through the same codec the handler issues it from.
    let first_cursor = inquiry_queries_token(&key, &id, 0);
    let resumed = list_queries(&router, &id, &format!("&nextToken={first_cursor}")).await;
    assert_eq!(
        resumed.queries().len(),
        1,
        "case \"a token resuming at the first query\": expected the query at that \
         identifier, got {:?}",
        resumed.body
    );
    assert_eq!(
        resumed.queries()[0]["query"].as_str(),
        Some(query),
        "case \"a token resuming at the first query\": expected {query:?}, got {:?}",
        resumed.body
    );
    let past_cursor = inquiry_queries_token(&key, &id, 1);
    let exhausted = list_queries(&router, &id, &format!("&nextToken={past_cursor}")).await;
    assert_eq!(
        exhausted.status, 200,
        "case \"a token past the last query\": expected HTTP 200, got {} with {:?}",
        exhausted.status, exhausted.body
    );
    assert!(
        exhausted.queries().is_empty(),
        "case \"a token past the last query\": expected an empty page, got {:?}",
        exhausted.body
    );
    assert!(
        exhausted.body["nextToken"].is_null(),
        "case \"a token past the last query\": nothing remains, so no nextToken is owed, \
         got {:?}",
        exhausted.body["nextToken"]
    );
}

#[tokio::test]
async fn a_token_this_operation_never_issued_is_refused_with_the_fixed_text() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let key = page_token_key(&engine);
    let mine = create(&router, &text_body("Mine", "Who audits the auditors?", None)).await;
    let other = create(&router, &text_body("Other", "Where are the backups?", None)).await;

    // Every row satisfies `NextTokenAttribute`'s modelled length, so each one reaches the
    // handler and is refused for what it IS rather than for how it is spelled. The rows
    // differ along the token's shape: a word, a bare number, a cursor cut for ANOTHER
    // inquiry, an inquiry id with no resume point, resume points outside the identifier
    // sequence, and the unencoded `<inquiryId>:<identifier>` spelling of a resume point,
    // which is a resource identifier rather than a cursor this operation issued.
    let rows = [
        ("a word", "nota".to_string()),
        ("a bare number", "1".to_string()),
        (
            "a cursor cut for another inquiry",
            inquiry_queries_token(&key, &other, 1),
        ),
        ("an unencoded resume point", format!("{mine}:1")),
        ("another inquiry's unencoded resume point", format!("{other}:1")),
        ("an inquiry id with no resume point", mine.clone()),
        ("a resume point below the sequence", format!("{mine}:0")),
        ("a negative resume point", format!("{mine}:-1")),
        ("a resume point that is not a number", format!("{mine}:first")),
        ("an inquiry-shaped id that is too short", "compliance-inquiry-1:1".to_string()),
    ];

    for (case, token) in rows {
        let refused = list_queries(&router, &mine, &format!("&nextToken={token}")).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a token this service never issued must be refused with HTTP 400, \
             got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.message(),
            INVALID_TOKEN,
            "case {case:?}: expected the fixed invalid-token text {INVALID_TOKEN:?}, got {:?}",
            refused.message()
        );
        assert_eq!(
            refused.body["reason"].as_str(),
            Some("invalidToken"),
            "case {case:?}: the refusal must report the invalidToken reason, got {:?}",
            refused.body
        );
    }
}

#[tokio::test]
async fn an_inquiry_this_account_holds_none_for_is_refused_by_the_front_door() {
    let router = emulator();

    // Well-formed `InquiryId`s (`^compliance-inquiry-[a-zA-Z0-9]{16}$`) the store does not
    // hold. The stored inquiry set IS the account's entitlement, and AWS Artifact is served
    // through a gateway that authorizes the targeted resource before the service is
    // reached, so a well-formed id naming no inquiry this account holds is answered with
    // the gateway's 401 rather than a modelled 404 that would confirm its absence — the
    // same answer `GetComplianceInquiryMetadata` gives for the same id.
    let rows = [
        ("hex digits", "compliance-inquiry-0123456789abcdef"),
        ("mixed case", "compliance-inquiry-ZZZZzzzz00001111"),
        ("all letters", "compliance-inquiry-abcdefghijklmnop"),
    ];

    for (case, id) in rows {
        let refused = list_queries(&router, id, "").await;
        assert_eq!(
            refused.status, 401,
            "case {case:?}: an inquiry this account holds none for must be refused with \
             HTTP 401, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "UnauthorizedException",
            "case {case:?}: expected UnauthorizedException, got {:?}",
            refused.body
        );
        assert!(
            !refused.message().contains(id),
            "case {case:?}: the refusal must not confirm anything about the id asked for, \
             got {:?}",
            refused.message()
        );
    }
}
