// SPDX-License-Identifier: CC0-1.0

//! `ListComplianceInquiries`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones the operation's model states:
//!
//! * the answer is COMPUTED from the persisted inquiries: an inquiry
//!   `CreateComplianceInquiry` created is reported as the same `InquirySummary` the create
//!   returned, whatever form the submission took, and an account that created none is
//!   reported as an empty list;
//! * the operation is `@readonly`, so listing changes nothing a later caller sees — the
//!   one transition it materialises is the one the CLOCK already owed the inquiry, which
//!   is why a listing and a `GetComplianceInquiryMetadata` of the same inquiry agree on
//!   its status;
//! * `@paginated(items: complianceInquiries)`: a page is cut at `maxResults`, a `nextToken`
//!   arrives exactly when inquiries remain, and it resumes where the previous page ended —
//!   so walking the pages at ANY admissible page size enumerates the whole set once;
//! * `nextToken` carries an OPAQUE cursor rather than an `InquiryId`, so a token this
//!   service never issued — a bare inquiry id included — is refused with the modelled
//!   `ValidationException`, while a cursor naming no stored inquiry still resumes at the
//!   first id at or after it.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::state::{
    inquiries_page_token, service_state, PageTokenKey,
};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The bounds `MaxResultsAttribute` declares (`@range(min: 1, max: 300)`), spelled out
/// here rather than imported from the crate so a wrong bound in `src/state.rs` fails
/// (`smithy_query(query="shape",shape="MaxResultsAttribute")`).
const MAX_RESULTS_FLOOR: i32 = 1;
/// See [`MAX_RESULTS_FLOOR`].
const MAX_RESULTS_CEILING: i32 = 300;

/// The values `InquiryStatus` declares, spelled out here rather than imported from the
/// crate so a status outside the enum fails this suite
/// (`smithy_query(query="shape",shape="InquiryStatus")`).
const INQUIRY_STATUSES: &[&str] = &["PROCESSING", "HUMAN_REVIEW", "COMPLETED", "FAILED"];

/// The values `InquiryStatusMessage` declares — it is an ENUM, so a `statusMessage` AWS
/// never emits fails this suite
/// (`smithy_query(query="shape",shape="InquiryStatusMessage")`).
const INQUIRY_STATUS_MESSAGES: &[&str] = &[
    "Compliance inquiry processing is complete.",
    "Malware was detected on the file. Provide a new file and try again.",
    "Compliance inquiry processing is in-progress.",
    "An internal error occurred while processing the inquiry. Try again at a later time.",
    "Human review is in progress.",
    "Compliance inquiry processing is complete. One or more queries encountered errors during processing.",
];

/// The span after which an inquiry's processing is due to be over, plus slack. The
/// handlers store the due instant rather than sleeping, so only the test waits.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio
/// without its `time` feature.
const PAST_THE_PROCESSING_SPAN: Duration = Duration::from_millis(900);

/// The region the suite signs for.
const REGION: &str = "eu-west-1";

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob.
const FILE_CONTENT: &str = "cXVlc3Rpb25uYWlyZQ==";

/// A SigV4 credential scope for `region` naming this service — what routes the request.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/artifact/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

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

    /// The `complianceInquiries` list a listing returned, or `None` when the member is
    /// missing altogether.
    fn inquiries(&self) -> Option<&Vec<Value>> {
        self.body["complianceInquiries"].as_array()
    }

    /// The reported inquiry ids, in the order they arrived.
    fn ids(&self) -> Vec<String> {
        self.inquiries()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item["id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The summary the listing reports for `id`.
    fn summary_of(&self, id: &str) -> Option<&Value> {
        self.inquiries()?
            .iter()
            .find(|item| item["id"].as_str() == Some(id))
    }

    /// The `nextToken` the answer carries, if any.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }
}

async fn send(router: &MultiServiceRouter, request: ::http::Request<Bytes>) -> Answer {
    let uri = request.uri().to_string();
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn create(router: &MultiServiceRouter, body: &Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/v1/compliance-inquiry/create")
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", signed_for(REGION))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    send(router, request).await
}

async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(format!("/v1/compliance-inquiry/list{query}"))
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("authorization", signed_for(REGION))
        .body(Bytes::new())
        .expect("the test request is well formed");
    send(router, request).await
}

async fn get_metadata(router: &MultiServiceRouter, inquiry_id: &str) -> Answer {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(format!(
            "/v1/compliance-inquiry/getMetadata?complianceInquiryId={inquiry_id}"
        ))
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("authorization", signed_for(REGION))
        .body(Bytes::new())
        .expect("the test request is well formed");
    send(router, request).await
}

/// `InquiryId`'s `^compliance-inquiry-[a-zA-Z0-9]{16}$`, checked here rather than through
/// the crate so a wrong predicate cannot make this test pass.
fn matches_inquiry_id_pattern(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("compliance-inquiry-") else {
        return false;
    };
    suffix.len() == 16 && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// A `CreateComplianceInquiry` body.
fn body(name: &str, inquiry_content: Value, support_mode: Option<&str>, tags: &[(&str, &str)]) -> Value {
    let mut body = json!({ "name": name, "inquiryContent": inquiry_content });
    if let Some(mode) = support_mode {
        body["supportMode"] = json!(mode);
    }
    if !tags.is_empty() {
        body["tags"] = Value::Object(
            tags.iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect(),
        );
    }
    body
}

/// One submission, and what the inquiry it creates must report once listed.
struct Case {
    label: &'static str,
    body: Value,
    /// `InputSource`: `TEXT` for a submitted query, `FILE` for an uploaded document.
    input_source: &'static str,
    /// The `InquiryStatus` the clock owes this inquiry, and the message that goes with
    /// it. A submission's units of work become its queries, so a submission that carries
    /// any reaches `COMPLETED` once the service's own model has processed them — the one
    /// support mode the model admits (`AI_ONLY`) is an AI review.
    terminal_status: &'static str,
    /// See [`Case::terminal_status`].
    terminal_message: &'static str,
}

/// The submissions this suite lists. The rows differ along everything the rule
/// generalises over: the form of the content (a query versus an uploaded document), how
/// many file sections it names, whether `supportMode` is given and which value, and how
/// many tags come with it — none of which may change whether the inquiry is reported.
fn cases() -> Vec<Case> {
    const COMPLETED: &str = "Compliance inquiry processing is complete.";
    vec![
        Case {
            label: "a query, no supportMode, no tags",
            body: body(
                "Data residency",
                json!({ "query": "Where is customer data stored?" }),
                None,
                &[],
            ),
            input_source: "TEXT",
            terminal_status: "COMPLETED",
            terminal_message: COMPLETED,
        },
        Case {
            label: "a query, AI_ONLY, three tags",
            body: body(
                "Subprocessors",
                json!({ "query": "Which subprocessors handle customer content?" }),
                Some("AI_ONLY"),
                &[("team", "legal"), ("env", "prod"), ("owner", "a.b@c")],
            ),
            input_source: "TEXT",
            terminal_status: "COMPLETED",
            terminal_message: COMPLETED,
        },
        Case {
            label: "an uploaded document, no sections",
            body: body(
                "Vendor questionnaire",
                json!({ "fileContent": { "content": FILE_CONTENT } }),
                None,
                &[],
            ),
            input_source: "FILE",
            terminal_status: "COMPLETED",
            terminal_message: COMPLETED,
        },
        Case {
            label: "an uploaded document, two sections, AI_ONLY, one tag",
            body: body(
                "Security addendum",
                json!({
                    "fileContent": {
                        "content": FILE_CONTENT,
                        "fileSections": ["Sheet1", "Sheet2"],
                    }
                }),
                Some("AI_ONLY"),
                &[("team", "security")],
            ),
            input_source: "FILE",
            terminal_status: "COMPLETED",
            terminal_message: COMPLETED,
        },
        Case {
            label: "an uploaded document, one section, AI_ONLY",
            body: body(
                "Annual audit workbook",
                json!({
                    "fileContent": {
                        "content": FILE_CONTENT,
                        "fileSections": ["Section A"],
                    }
                }),
                Some("AI_ONLY"),
                &[],
            ),
            input_source: "FILE",
            terminal_status: "COMPLETED",
            terminal_message: COMPLETED,
        },
    ]
}

/// Create every case on `router`, returning each case's label beside the
/// `complianceInquirySummary` its create returned.
async fn create_every_case(router: &MultiServiceRouter) -> Vec<(&'static str, Value)> {
    let mut created = Vec::new();
    for case in cases() {
        let answer = create(router, &case.body).await;
        assert_eq!(
            answer.status, 202,
            "case {:?}: expected HTTP 202 from CreateComplianceInquiry, got {} with {:?}",
            case.label, answer.status, answer.body
        );
        let summary = answer.body["complianceInquirySummary"].clone();
        assert!(
            summary.is_object(),
            "case {:?}: the create must return a summary, got {:?}",
            case.label,
            answer.body
        );
        created.push((case.label, summary));
    }
    created
}

/// Walk every page at `page_size`, returning the concatenated summaries.
///
/// Asserts the token rule as it goes: a page is never longer than the requested size, and
/// a `nextToken` only ever arrives with a full page.
async fn walk(router: &MultiServiceRouter, page_size: i32, total: usize) -> Vec<Value> {
    let size = usize::try_from(page_size).expect("a positive page size");
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..=total {
        let query = match token.as_deref() {
            None => format!("?maxResults={page_size}"),
            Some(value) => format!("?maxResults={page_size}&nextToken={value}"),
        };
        let answer = list(router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={page_size} page {page}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let items = answer
            .inquiries()
            .unwrap_or_else(|| {
                panic!(
                    "case maxResults={page_size} page {page}: complianceInquiries must be a \
                     list, got {:?}",
                    answer.body
                )
            })
            .clone();
        assert!(
            items.len() <= size,
            "case maxResults={page_size} page {page}: a page must hold at most {page_size} \
             inquiries, got {}",
            items.len()
        );
        collected.extend(items.iter().cloned());
        match answer.next_token() {
            None => return collected,
            Some(next) => {
                assert_eq!(
                    items.len(),
                    size,
                    "case maxResults={page_size} page {page}: a nextToken must only arrive with \
                     a full page, got {} items",
                    items.len()
                );
                token = Some(next);
            }
        }
    }
    panic!("case maxResults={page_size}: pagination did not terminate over {total} inquiries");
}

#[tokio::test]
async fn an_account_that_created_no_inquiry_is_reported_as_an_empty_list() {
    let router = emulator();
    let answer = list(&router, "").await;
    assert_eq!(
        answer.status, 200,
        "expected HTTP 200 from ListComplianceInquiries, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.inquiries().map(Vec::len),
        Some(0),
        "an account with no inquiry must be reported as an empty list, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.next_token(),
        None,
        "an empty page must carry no nextToken, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn every_created_inquiry_is_reported_as_the_summary_the_create_returned() {
    let router = emulator();
    let created = create_every_case(&router).await;

    let answer = list(&router, "").await;
    assert_eq!(
        answer.status, 200,
        "expected HTTP 200 from ListComplianceInquiries, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.inquiries().map(Vec::len),
        Some(created.len()),
        "every created inquiry must be reported, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.next_token(),
        None,
        "a page holding every inquiry must carry no nextToken, got {:?}",
        answer.body
    );

    // Creation order, which is the store's stable total order over ascending ids: the
    // page a token indexes into.
    let reported = answer.ids();
    let expected_order: Vec<String> = created
        .iter()
        .map(|(_, summary)| summary["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported, expected_order,
        "inquiries must be reported in creation order, got {reported:?}"
    );
    let mut ascending = reported.clone();
    ascending.sort();
    assert_eq!(
        reported, ascending,
        "creation order must be ascending id order, got {reported:?}"
    );
    assert_eq!(
        reported.iter().collect::<BTreeSet<_>>().len(),
        reported.len(),
        "no inquiry may be reported twice, got {reported:?}"
    );

    for (case, created_summary) in &created {
        let id = created_summary["id"].as_str().unwrap_or_default();
        let listed = answer
            .summary_of(id)
            .unwrap_or_else(|| panic!("case {case:?}: {id} must be reported, got {:?}", answer.body));
        assert!(
            matches_inquiry_id_pattern(id),
            "case {case:?}: id must match ^compliance-inquiry-[a-zA-Z0-9]{{16}}$, got {id:?}"
        );
        // Every member of the projection that does not move with the clock is the one the
        // create returned, because both read the one stored record.
        for member in ["arn", "name", "id", "inputSource", "createdAt"] {
            assert_eq!(
                listed[member], created_summary[member],
                "case {case:?}: expected {member} {:?} from the create, got {:?}",
                created_summary[member], listed[member]
            );
        }
        let status = listed["status"].as_str().unwrap_or_default();
        assert!(
            INQUIRY_STATUSES.contains(&status),
            "case {case:?}: expected an InquiryStatus value, got {status:?}"
        );
        let message = listed["statusMessage"].as_str().unwrap_or_default();
        assert!(
            INQUIRY_STATUS_MESSAGES.contains(&message),
            "case {case:?}: expected an InquiryStatusMessage value, got {message:?}"
        );
    }

    // The input source each submission was made through, by case.
    for (case, expected) in cases().iter().map(|row| (row.label, row.input_source)) {
        let created_summary = &created
            .iter()
            .find(|(label, _)| *label == case)
            .expect("every case was created")
            .1;
        let id = created_summary["id"].as_str().unwrap_or_default();
        let listed = answer.summary_of(id).expect("the inquiry is reported");
        assert_eq!(
            listed["inputSource"].as_str(),
            Some(expected),
            "case {case:?}: expected inputSource {expected:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn listing_reports_the_transition_the_clock_owed_and_changes_nothing_else() {
    let router = emulator();
    let created = create_every_case(&router).await;

    // Before the due instant the inquiry is still the non-terminal resource the create
    // returned, so a listing reports exactly that.
    let fresh = list(&router, "").await;
    for (case, created_summary) in &created {
        let id = created_summary["id"].as_str().unwrap_or_default();
        let listed = fresh
            .summary_of(id)
            .unwrap_or_else(|| panic!("case {case:?}: {id} must be reported"));
        if created_summary["status"].as_str() == Some("PROCESSING")
            && listed["status"].as_str() == Some("PROCESSING")
        {
            assert_eq!(
                listed, created_summary,
                "case {case:?}: a listing before the due instant must report the created \
                 record, got {listed:?}"
            );
        }
    }

    std::thread::sleep(PAST_THE_PROCESSING_SPAN);

    let after = list(&router, "").await;
    for (case, status, message) in cases()
        .iter()
        .map(|row| (row.label, row.terminal_status, row.terminal_message))
    {
        let created_summary = &created
            .iter()
            .find(|(label, _)| *label == case)
            .expect("every case was created")
            .1;
        let id = created_summary["id"].as_str().unwrap_or_default();
        let listed = after
            .summary_of(id)
            .unwrap_or_else(|| panic!("case {case:?}: {id} must be reported"));
        assert_eq!(
            listed["status"].as_str(),
            Some(status),
            "case {case:?}: expected the clock-owed status {status:?}, got {listed:?}"
        );
        assert_eq!(
            listed["statusMessage"].as_str(),
            Some(message),
            "case {case:?}: expected the clock-owed statusMessage {message:?}, got {listed:?}"
        );

        // The listing and the metadata read answer from the ONE stored record, so they
        // cannot disagree about it.
        let detail = get_metadata(&router, id).await;
        assert_eq!(
            detail.status, 200,
            "case {case:?}: expected HTTP 200 from GetComplianceInquiryMetadata, got {} with {:?}",
            detail.status, detail.body
        );
        for member in ["arn", "name", "id", "status", "statusMessage", "inputSource"] {
            assert_eq!(
                detail.body["complianceInquiryDetail"][member], listed[member],
                "case {case:?}: the listing and the metadata read must agree on {member}, got \
                 {:?} and {:?}",
                detail.body["complianceInquiryDetail"][member], listed[member]
            );
        }
    }

    // A read changes nothing a later caller sees: the next listing repeats this one.
    let again = list(&router, "").await;
    assert_eq!(
        again.inquiries(),
        after.inquiries(),
        "two successive listings must report the same records; first {:?}, second {:?}",
        after.body,
        again.body
    );
}

#[tokio::test]
async fn pagination_enumerates_the_set_once_at_every_admissible_page_size() {
    let router = emulator();
    let created = create_every_case(&router).await;
    // Past the due instant, so every page of every walk projects a settled record and the
    // walks are comparable with each other.
    std::thread::sleep(PAST_THE_PROCESSING_SPAN);

    let whole = list(&router, &format!("?maxResults={MAX_RESULTS_CEILING}")).await;
    let expected = whole
        .inquiries()
        .unwrap_or_else(|| panic!("complianceInquiries must be a list, got {:?}", whole.body))
        .clone();
    assert_eq!(
        expected.len(),
        created.len(),
        "the ceiling page must hold every inquiry, got {:?}",
        whole.body
    );

    // Every page size the model admits that can cut this set, plus both published bounds.
    let mut sizes: Vec<i32> = (MAX_RESULTS_FLOOR
        ..=i32::try_from(created.len() + 1).expect("a small set"))
        .chain([MAX_RESULTS_CEILING])
        .collect();
    sizes.dedup();
    for page_size in sizes {
        let walked = walk(&router, page_size, created.len()).await;
        assert_eq!(
            walked, expected,
            "case maxResults={page_size}: walking the pages must enumerate the whole set once, \
             got {walked:?}"
        );
    }

    // Absent `maxResults` serves the largest page the model allows, so the whole set.
    let defaulted = list(&router, "").await;
    assert_eq!(
        defaulted.inquiries().cloned(),
        Some(expected),
        "an absent maxResults must serve the whole set, got {:?}",
        defaulted.body
    );
}

#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let router = emulator();
    create_every_case(&router).await;
    // `MaxResultsAttribute` is `@range(min: 1, max: 300)`: each bound is served, and one
    // step past either is refused at the boundary.
    for accepted in [MAX_RESULTS_FLOOR, MAX_RESULTS_CEILING] {
        let answer = list(&router, &format!("?maxResults={accepted}")).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={accepted}: a bound of the modelled range must be served, got {} \
             with {:?}",
            answer.status, answer.body
        );
        let served = answer.inquiries().map(Vec::len).unwrap_or_default();
        assert!(
            served <= usize::try_from(accepted).expect("a positive page size"),
            "case maxResults={accepted}: a page must hold at most {accepted} inquiries, got \
             {served}"
        );
    }
    for refused_value in [MAX_RESULTS_FLOOR - 1, MAX_RESULTS_CEILING + 1] {
        let refused = list(&router, &format!("?maxResults={refused_value}")).await;
        assert_eq!(
            refused.status, 400,
            "case maxResults={refused_value}: a value outside 1..300 must be refused, got {} \
             with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case maxResults={refused_value}: expected ValidationException, got {:?}",
            refused.body
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let key = page_token_key(&engine);
    create_every_case(&router).await;

    // A `nextToken` is an OPAQUE cursor this operation cut, so every row here is a value
    // that is not one: a made-up string, a token of another operation, and an id whose
    // suffix is the wrong length or carries characters the pattern excludes.
    for token in [
        "not-a-token",
        "0",
        "compliance-inquiry-",
        "compliance-inquiry-short",
        "compliance-inquiry-waytoolongtobeanid",
        "compliance-inquiry-not_alphanumeric",
        "report-cloudsecalliance",
        "customer-agreement-baaagreement0001",
    ] {
        let refused = list(&router, &format!("?nextToken={token}")).await;
        assert_eq!(
            refused.status, 400,
            "case {token:?}: an unissued nextToken must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {:?}",
            refused.body
        );
    }

    let all = list(&router, "").await.ids();
    assert!(
        all.len() > 1,
        "the suite needs several inquiries to resume between, got {all:?}"
    );

    // The case this operation used to serve: a BARE inquiry id, even one the store holds,
    // is a resource identifier rather than a cursor, so it was never issued as a token.
    for id in &all {
        let refused = list(&router, &format!("?nextToken={id}")).await;
        assert_eq!(
            refused.status, 400,
            "case {id:?}: a bare inquiry id is not a token this operation issued, got {} \
             with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {id:?}: expected ValidationException, got {:?}",
            refused.body
        );
    }

    // The token this operation DOES issue carries none of the store's identifiers in the
    // clear, and stays inside `NextTokenAttribute`'s `@length(1..2048)`.
    let issued = list(&router, "?maxResults=1")
        .await
        .next_token()
        .expect("a store of several inquiries must page");
    assert!(
        !issued.is_empty() && issued.len() <= 2048,
        "case \"the issued token\": NextTokenAttribute is @length(1..2048), got {} bytes",
        issued.len()
    );
    for id in &all {
        assert!(
            &issued != id && !issued.contains(id.as_str()),
            "case \"the issued token\": a pagination token must not be or carry the \
             identifier {id:?}, got {issued:?}"
        );
    }

    // And the token taken from an ACTUAL page resumes the listing at the inquiry after
    // that page, so the cursor rows below cannot pass on a symmetric encode/decode defect.
    let resumed = list(&router, &format!("?nextToken={issued}")).await;
    assert_eq!(
        resumed.status, 200,
        "case \"the issued token\": a token this operation cut must be served, got {} with {:?}",
        resumed.status, resumed.body
    );
    assert_eq!(
        resumed.ids(),
        all[1..].to_vec(),
        "case \"the issued token\": expected the inquiries after the first page, got {:?}",
        resumed.body
    );

    // A cursor this operation cut resumes the listing at the POSITION it carries, so the
    // page after a cursor is exactly the tail of the reported order behind it. Every
    // position is covered — including one past the last inquiry, which serves an empty
    // page rather than refusing. The expected tail is computed from the whole set, which
    // is the rule rather than one recorded page.
    for position in 0..=all.len() {
        let expected: Vec<String> = all[position..].to_vec();
        let token = inquiries_page_token(&key, position);
        let answer = list(&router, &format!("?nextToken={token}")).await;
        assert_eq!(
            answer.status, 200,
            "case position={position}: a cursor this operation cut must be served, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.ids(),
            expected,
            "case position={position}: expected the inquiries after the cursor, got {:?}",
            answer.body
        );
    }
}
