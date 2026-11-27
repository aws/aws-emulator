// SPDX-License-Identifier: CC0-1.0

//! `ListTestTemplates`' `@paginated` window, end to end over HTTP.
//!
//! The rule under test: a `@paginated` listing serves at most `maxResults` items per page
//! and resumes at the position its previous page's `nextToken` names, so successive pages
//! enumerate every row of the collection exactly once in the collection's own order, only
//! a page that leaves rows behind carries a token, `maxResults` is held to its modelled
//! `range 1..100`, and a `nextToken` this service did not issue is refused as a 400
//! `ValidationException` rather than answered with a page.
//!
//! Every case drives the real wire path (`MultiServiceRouter::dispatch`), so the modelled
//! constraints, the restJson1 (de)serializers and the HTTP status codes are exercised the
//! way a client exercises them.
//!
//! The suites are TABLES: rows differ along the requested page size (every size from one
//! to one past the catalog, plus each published bound), along the position a token resumes
//! at, and along the shape of a forged token.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::state::TEST_TEMPLATE_CATALOG;
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The operation's modelled route: `GET /v2/list-test-templates`.
const LIST_TEST_TEMPLATES: &str = "/v2/list-test-templates";

/// The Region these suites sign for.
const REGION: &str = "us-east-1";

/// `MaxResults`' modelled range, which `ListTestTemplates$maxResults` targets.
///
/// SOURCE: `smithy_query(query="operation", operation="ListTestTemplates")` —
/// `range {min: 1, max: 100}`, `default 100`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These requests carry an unverifiable placeholder signature: these suites test the
    // operation, not SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing name
/// and Region. Signature verification is off for these tests, so the value only has to
/// parse.
fn signed() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY00000000/20260101/{REGION}\
         /resiliencehub/aws4_request, SignedHeaders=host;x-amz-date, \
         Signature=0123456789abcdef"
    )
}

/// Dispatch `GET /v2/list-test-templates?{query}` and answer its status and body.
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    let uri = if query.is_empty() {
        LIST_TEST_TEMPLATES.to_string()
    } else {
        format!("{LIST_TEST_TEMPLATES}?{query}")
    };
    let request = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", signed())
        .body(bytes::Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, body)
}

/// The modelled error code of a refusal, from the restJson1 `__type` discriminator.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `name` of every summary on a successful page, in the order the page listed them.
fn page_names(case: &str, status: u16, body: &Value) -> Vec<String> {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} {body:?}"
    );
    body["testTemplates"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the @required testTemplates member is absent from {body:?}")
        })
        .iter()
        .map(|summary| {
            summary["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no name in {summary:?}"))
                .to_string()
        })
        .collect()
}

/// Every catalogued template's name, in the order the catalog holds them — the order the
/// pages must enumerate.
fn catalogued_names() -> Vec<String> {
    TEST_TEMPLATE_CATALOG
        .iter()
        .map(|entry| entry.name.to_string())
        .collect()
}

#[tokio::test]
async fn pagination_walks_every_template_exactly_once() {
    let router = emulator();
    let expected = catalogued_names();

    // The rows exhaust every page size from one to one past the number of catalogued
    // templates, plus the published `maxResults` maximum: a page size smaller than, equal
    // to and larger than the collection.
    let mut sizes: Vec<usize> = (1..=expected.len() + 1).collect();
    sizes.push(MAX_RESULTS_MAX as usize);

    for page_size in sizes {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query = format!("maxResults={page_size}");
            if let Some(cursor) = &token {
                query.push_str(&format!("&nextToken={cursor}"));
            }
            let (status, body) = list(&router, &query).await;
            let names = page_names(&case, status, &body);
            let remaining = expected.len() - seen.len();
            assert_eq!(
                names.len(),
                page_size.min(remaining),
                "case {case:?}: page {pages} must be full while templates remain, got {names:?}"
            );
            for name in &names {
                assert!(
                    !seen.contains(name),
                    "case {case:?}: {name:?} appeared on two pages, already seen {seen:?}"
                );
            }
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            match body["nextToken"].as_str() {
                Some(cursor) => {
                    assert!(
                        seen.len() < expected.len(),
                        "case {case:?}: a token was issued after the last template, got {body:?}"
                    );
                    token = Some(cursor.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: the pages must enumerate every catalogued template in the \
             catalog's order"
        );
    }
}

#[tokio::test]
async fn a_request_with_no_page_size_serves_the_whole_catalog_on_one_page() {
    let router = emulator();
    let expected = catalogued_names();

    // `maxResults` is modelled with a default of 100, which the whole catalog fits inside,
    // so a request that names no page size is served in one page and carries no token.
    for (case, query) in [
        ("no query at all", ""),
        ("the modelled default page size", "maxResults=100"),
    ] {
        let (status, body) = list(&router, query).await;
        assert_eq!(
            page_names(case, status, &body),
            expected,
            "case {case:?}: expected the whole catalog on one page, got {body:?}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: a page that leaves no template behind carries no nextToken, \
             got {body:?}"
        );
    }
}

#[tokio::test]
async fn max_results_is_held_to_its_published_range() {
    let router = emulator();

    for accepted in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let (status, body) = list(&router, &format!("maxResults={accepted}")).await;
        assert_eq!(
            status, 200,
            "case maxResults={accepted}: expected HTTP 200, got {status} {body:?}"
        );
    }
    for refused in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let (status, body) = list(&router, &format!("maxResults={refused}")).await;
        assert_eq!(
            status, 400,
            "case maxResults={refused}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case maxResults={refused}: expected ValidationException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_page_token_resumes_the_same_scan_and_is_stable() {
    let router = emulator();
    let expected = catalogued_names();

    // The rows differ along the position the token was cut at: after the first template,
    // and after all but the last one.
    for page_size in [1usize, expected.len() - 1] {
        let case = format!("a token cut after {page_size} template(s)");
        let (status, first) = list(&router, &format!("maxResults={page_size}")).await;
        assert_eq!(
            page_names(&case, status, &first),
            expected[..page_size].to_vec(),
            "case {case:?}: the first page must be the catalog's first rows, got {first:?}"
        );
        let token = first["nextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no nextToken in {first:?}"))
            .to_string();

        // A token is opaque but whitespace-free, as `NextToken`'s modelled
        // `^\S{1,2000}$` requires.
        assert!(
            !token.is_empty() && !token.chars().any(char::is_whitespace) && token.len() <= 2000,
            "case {case:?}: expected an opaque whitespace-free token, got {token:?}"
        );

        // Resuming names the rest of the catalog, and resuming the SAME token twice reads
        // the same page: a listing changes nothing a later caller sees.
        let query = format!("maxResults={}&nextToken={token}", expected.len());
        let (status, resumed) = list(&router, &query).await;
        assert_eq!(
            page_names(&case, status, &resumed),
            expected[page_size..].to_vec(),
            "case {case:?}: a token must resume at the position it names, got {resumed:?}"
        );
        let (status, again) = list(&router, &query).await;
        assert_eq!(
            page_names(&case, status, &again),
            page_names(&case, status, &resumed),
            "case {case:?}: replaying a token must answer identically"
        );
    }
}

#[tokio::test]
async fn a_forged_page_token_is_refused() {
    let router = emulator();

    // A `nextToken` must be one this service issued. The rows span a token that is not the
    // encoding at all, one that encodes a position before the first (positions are
    // 1-based), and one that encodes a position past the last row of the collection — hex
    // of the decimal position being the form this service issues.
    let past_the_end = ::hex::encode((TEST_TEMPLATE_CATALOG.len() + 1).to_string());
    let forged: [&str; 5] = [
        "zzzz",
        "0",
        "not-a-token",
        // Hex of "0": a well-formed token naming a position before the first.
        "30",
        &past_the_end,
    ];

    for token in forged {
        let (status, body) = list(&router, &format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case nextToken={token}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case nextToken={token}: expected ValidationException, got {body:?}"
        );
    }
}
