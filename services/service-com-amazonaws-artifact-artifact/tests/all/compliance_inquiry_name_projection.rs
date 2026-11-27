// SPDX-License-Identifier: CC0-1.0

//! The NAME every read of a compliance inquiry reports, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is one rule, asserted over a set of submissions:
//!
//! * an inquiry's reported name is DERIVED FROM ITS CONTENT when the content derives one —
//!   a submission made of a single text query is reported under that query text — and the
//!   caller-supplied title stands as the reported name only for content that derives none
//!   (an uploaded document, whose queries are written inside the file);
//! * the rule is ONE projection, so `CreateComplianceInquiry`,
//!   `ListComplianceInquiries` and `GetComplianceInquiryMetadata` cannot disagree about
//!   what an inquiry is called, at any page size;
//! * the projection follows the CONTENT, so the title a caller submits never changes what
//!   a text inquiry is reported as, and a query is reported in full at every length its
//!   model admits.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The bounds `InquiryContent$query` declares through `LongStringAttribute`
/// (`@length(min: 1, max: 2048)`), spelled out here rather than imported from the crate so
/// a wrong bound cannot make this suite pass
/// (`smithy_query(query="shape",shape="LongStringAttribute")`).
const QUERY_LENGTH_FLOOR: usize = 1;
/// See [`QUERY_LENGTH_FLOOR`].
const QUERY_LENGTH_CEILING: usize = 2048;

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

impl Answer {
    /// The `complianceInquirySummary` a create returned.
    fn summary(&self) -> &Value {
        &self.body["complianceInquirySummary"]
    }

    /// The `complianceInquiryDetail` a metadata read returned.
    fn detail(&self) -> &Value {
        &self.body["complianceInquiryDetail"]
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

/// Every stored inquiry, gathered by following `nextToken` at `page_size` — the walk a
/// client performs when it looks for its own inquiry in the listing.
async fn every_listed_inquiry(router: &MultiServiceRouter, page_size: i32) -> Vec<Value> {
    let mut gathered = Vec::new();
    let mut token: Option<String> = None;
    // A bound on the walk, so a pagination bug ends this test rather than hanging it.
    for _ in 0..64 {
        let query = match &token {
            Some(next) => format!("?maxResults={page_size}&nextToken={next}"),
            None => format!("?maxResults={page_size}"),
        };
        let answer = list(router, &query).await;
        assert_eq!(
            answer.status, 200,
            "paging at {page_size}: a listing must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        gathered.extend(
            answer.body["complianceInquiries"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        token = answer.next_token();
        if token.is_none() {
            return gathered;
        }
    }
    panic!("paging at {page_size}: the listing never ran out of pages");
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

/// One submission, and the name every read of the inquiry it creates must report.
struct Case {
    label: &'static str,
    body: Value,
    /// `InputSource`: `TEXT` for a submitted query, `FILE` for an uploaded document.
    input_source: &'static str,
    /// What the rule projects: the query text for content that derives a name, the
    /// submitted title for content that derives none.
    expected_name: String,
}

/// One submission made of a single text query: the content derives the reported name, so
/// it is the query itself whatever the title says.
fn text(
    label: &'static str,
    title: &str,
    query: &str,
    support_mode: Option<&str>,
    tags: &[(&str, &str)],
) -> Case {
    Case {
        label,
        body: body(title, json!({ "query": query }), support_mode, tags),
        input_source: "TEXT",
        expected_name: query.to_string(),
    }
}

/// One submission made of an uploaded document: its queries are written inside the file,
/// so the content derives no name and the submitted title is what is reported.
fn file(
    label: &'static str,
    title: &str,
    sections: &[&str],
    support_mode: Option<&str>,
    tags: &[(&str, &str)],
) -> Case {
    let mut content = json!({ "content": FILE_CONTENT });
    if !sections.is_empty() {
        content["fileSections"] = json!(sections);
    }
    Case {
        label,
        body: body(title, json!({ "fileContent": content }), support_mode, tags),
        input_source: "FILE",
        expected_name: title.to_string(),
    }
}

/// The submissions this suite covers. The rows differ along everything the rule
/// generalises over: the form of the content (a query versus an uploaded document), the
/// query's length at each published bound and in between, its character content
/// (ASCII, punctuation, non-Latin script, surrounding whitespace), whether the query
/// happens to equal the title, how many file sections the document names, whether
/// `supportMode` is given and which value, and how many tags come with the submission.
fn cases() -> Vec<Case> {
    // The shortest and the longest query `LongStringAttribute` admits, built here so the
    // bounds are exercised rather than described. `@pattern ^[^<>]*$`, so no angle
    // brackets.
    let shortest = "?".repeat(QUERY_LENGTH_FLOOR);
    let longest = "a".repeat(QUERY_LENGTH_CEILING);
    vec![
        text(
            "a plain query, no supportMode, no tags",
            "quarterly SOC 2 review",
            "Does the SOC 2 report cover eu-west-1?",
            None,
            &[],
        ),
        text(
            "a query with punctuation and digits, AI_ONLY, one tag",
            "pci-dss",
            "Is PCI-DSS v4.0 in scope (Q3/2026)? 100% of regions, please!",
            Some("AI_ONLY"),
            &[("team", "compliance")],
        ),
        text(
            "a query in a non-Latin script, two tags",
            "encryption",
            "データは転送中に暗号化されますか?",
            Some("AI_ONLY"),
            &[("team", "legal"), ("env", "prod")],
        ),
        text(
            "a query surrounded by whitespace, which is projected verbatim",
            "whitespace",
            "  Which controls cover key rotation?  ",
            None,
            &[],
        ),
        text(
            "a query that equals the submitted title",
            "Is customer data encrypted at rest?",
            "Is customer data encrypted at rest?",
            None,
            &[],
        ),
        text(
            "the shortest query the model admits",
            "one character",
            &shortest,
            None,
            &[],
        ),
        text(
            "the longest query the model admits",
            "two thousand and forty-eight characters",
            &longest,
            Some("AI_ONLY"),
            &[("size", "max")],
        ),
        file(
            "an uploaded document, no sections, no tags",
            "vendor questionnaire",
            &[],
            None,
            &[],
        ),
        file(
            "an uploaded document, two sections, AI_ONLY, one tag",
            "annual audit workbook",
            &["Sheet1", "Sheet2"],
            Some("AI_ONLY"),
            &[("team", "audit")],
        ),
        file(
            "an uploaded document whose title reads like a query",
            "Does the SOC 2 report cover eu-west-1?",
            &["Controls"],
            Some("AI_ONLY"),
            &[],
        ),
    ]
}

#[tokio::test]
async fn an_inquiry_is_reported_under_the_name_its_content_derives() {
    let router = emulator();

    let mut ids = BTreeSet::new();
    let mut expected: Vec<(String, &'static str, String)> = Vec::new();

    for case in cases() {
        let label = case.label;
        let created = create(&router, &case.body).await;
        assert_eq!(
            created.status, 202,
            "case {label:?}: a create must answer 202, got {} with {:?}",
            created.status, created.body
        );
        let summary = created.summary().clone();
        assert_eq!(
            summary["inputSource"].as_str(),
            Some(case.input_source),
            "case {label:?}: expected inputSource {:?}, got {:?}",
            case.input_source,
            summary["inputSource"]
        );
        assert_eq!(
            summary["name"].as_str(),
            Some(case.expected_name.as_str()),
            "case {label:?}: expected the created summary's name {:?}, got {:?}",
            case.expected_name,
            summary["name"]
        );
        let id = summary["id"].as_str().unwrap_or_default().to_string();
        assert!(
            ids.insert(id.clone()),
            "case {label:?}: every create must mint a distinct id, got {id:?} twice"
        );

        // The same projection, read back through the detail: the create and the metadata
        // read answer the one stored record.
        let read = get_metadata(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetComplianceInquiryMetadata must find the created inquiry, \
             got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.detail()["name"].as_str(),
            Some(case.expected_name.as_str()),
            "case {label:?}: expected the detail's name {:?}, got {:?}",
            case.expected_name,
            read.detail()["name"]
        );

        expected.push((id, case.input_source, case.expected_name));
    }

    // The listing carries the same projection, at every admissible page size — a client
    // looking for the inquiry it just created finds it under the name the rule projects,
    // whatever it pages at.
    for page_size in [1, 3, 300] {
        let listed = every_listed_inquiry(&router, page_size).await;
        for (id, input_source, name) in &expected {
            let item = listed
                .iter()
                .find(|item| item["id"].as_str() == Some(id.as_str()))
                .unwrap_or_else(|| {
                    panic!(
                        "paging at {page_size}: the inquiry {id:?} must be listed, got \
                         {listed:?}"
                    )
                });
            assert_eq!(
                item["name"].as_str(),
                Some(name.as_str()),
                "paging at {page_size}, inquiry {id:?} ({input_source}): expected the \
                 listed name {name:?}, got {:?}",
                item["name"]
            );
            // The findability the summary's only content-derived handle buys: an entry
            // whose name IS the projected name, naming this inquiry.
            let found: Vec<&str> = listed
                .iter()
                .filter(|item| item["name"].as_str() == Some(name.as_str()))
                .map(|item| item["id"].as_str().unwrap_or_default())
                .collect();
            assert!(
                found.contains(&id.as_str()),
                "paging at {page_size}: scanning the listing for the name {name:?} must \
                 find {id:?}, found {found:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_submitted_title_never_changes_what_a_text_inquiry_is_reported_as() {
    let router = emulator();

    // One query, submitted under titles that differ along length, script and whether they
    // look like a query: the content derives the name, so the title cannot move it.
    const QUERY: &str = "Which regions does the ISO 27001 certificate cover?";
    let titles = [
        "iso-27001",
        "A rather long human-written title for the same question",
        "ISO",
        "Which regions does the ISO 27001 certificate cover?",
        "認証の範囲",
    ];

    let mut ids = BTreeSet::new();
    for title in titles {
        let created = create(&router, &body(title, json!({ "query": QUERY }), None, &[])).await;
        assert_eq!(
            created.status, 202,
            "title {title:?}: a create must answer 202, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.summary()["name"].as_str(),
            Some(QUERY),
            "title {title:?}: expected the query {QUERY:?} as the reported name, got {:?}",
            created.summary()["name"]
        );
        let id = created.summary()["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            ids.insert(id.clone()),
            "title {title:?}: a different title is a different submission, so it must \
             create a distinct inquiry, got {id:?} twice"
        );
    }

    // Every one of them is listed under that same query text, once each.
    let listed = every_listed_inquiry(&router, 300).await;
    let under_the_query: BTreeSet<String> = listed
        .iter()
        .filter(|item| item["name"].as_str() == Some(QUERY))
        .map(|item| item["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        under_the_query, ids,
        "every inquiry created from the query must be listed under it, expected {ids:?}, \
         got {under_the_query:?}"
    );
}

#[tokio::test]
async fn a_query_is_reported_in_full_at_every_admissible_length() {
    let router = emulator();

    // Each published bound of `LongStringAttribute`'s `@length(min: 1, max: 2048)`, one
    // inside each, and the one just under the ceiling: the whole query text is the name,
    // so nothing is truncated, padded or trimmed at any admissible length.
    let lengths = [
        QUERY_LENGTH_FLOOR,
        QUERY_LENGTH_FLOOR + 1,
        QUERY_LENGTH_CEILING / 2,
        QUERY_LENGTH_CEILING - 1,
        QUERY_LENGTH_CEILING,
    ];

    for length in lengths {
        // A query whose every character is admissible under `@pattern ^[^<>]*$`, built so
        // its length is exactly `length` and a truncation anywhere shows up as a shorter
        // name rather than an equal one.
        let query: String = (0..length)
            .map(|index| char::from(b'a' + u8::try_from(index % 26).unwrap_or(0)))
            .collect();
        let created = create(&router, &body("bounds", json!({ "query": query }), None, &[])).await;
        assert_eq!(
            created.status, 202,
            "length {length}: a query at an admissible length must be accepted, got {} \
             with {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.summary()["name"].as_str(),
            Some(query.as_str()),
            "length {length}: expected the whole query text as the reported name, got a \
             name of {:?} characters",
            created.summary()["name"].as_str().map(str::len)
        );
        let id = created.summary()["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let listed = every_listed_inquiry(&router, 300).await;
        let found: Vec<&str> = listed
            .iter()
            .filter(|item| item["name"].as_str() == Some(query.as_str()))
            .map(|item| item["id"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            found,
            vec![id.as_str()],
            "length {length}: scanning the listing for the submitted query must find \
             exactly {id:?}, found {found:?}"
        );
    }
}
