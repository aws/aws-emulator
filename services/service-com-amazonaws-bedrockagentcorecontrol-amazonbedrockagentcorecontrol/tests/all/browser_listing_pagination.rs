// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListBrowsers`' paging.
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what `CreateBrowser` writes is what the listing reads.
//!
//! The rule under test: one page of a browser listing holds at most as many summaries as
//! the caller's `maxResults` — or, when the caller names none, as many as that member's
//! own documentation gives as its default — and never more than the maximum that same
//! documentation states ("The default value is 10. The maximum value is 50.", from
//! `smithy_query(query="doc", shape="ListBrowsersRequest", member="maxResults")`, the only
//! source for either since the model declares no `@default`), in one ascending `browserId`
//! order, resumed strictly after the entry the `nextToken` it handed back was issued for,
//! so the pages of any size concatenate to the whole listing with every browser reported
//! exactly once; the modelled range 1..100 is what decides which `maxResults` values are
//! accepted, so one outside it — and a `nextToken` this service never issued — is the
//! modelled 400.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region every request in this suite signs for.
const REGION: &str = "eu-west-2";

/// The browser tool the SERVICE provides in every account, which `ResourceType`'s `SYSTEM`
/// member names (`smithy_query(query="shape", shape="ResourceType")`). It is listed beside
/// the account's own browsers whenever the caller filters by no type.
const SYSTEM_BROWSER_ID: &str = "aws.browser.v1";

/// The modelled bounds of `maxResults` (`MaxResults`, range 1..100, from
/// `smithy_query(query="operation", operation="ListBrowsers")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// The page sizes `maxResults`' own documentation states — "The maximum number of results
/// to return in a single call. The default value is 10. The maximum value is 50."
/// (`smithy_query(query="doc", shape="ListBrowsersRequest", member="maxResults")`): the
/// page a caller naming no `maxResults` gets, and the most any one page holds.
const DOCUMENTED_DEFAULT_PAGE: usize = 10;
const DOCUMENTED_MAX_PAGE: usize = 50;

/// How many browsers this account creates: more than the documented default page, so that
/// an unpaged listing has something left over to resume from.
const CREATED_BROWSERS: usize = 12;

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service, which is what the router routes on.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `ListBrowsers`: `POST /browsers`, with `type`, `maxResults` and `nextToken` on the
/// query string (`smithy_query(query="operation", operation="ListBrowsers")`).
async fn list_browsers(
    router: &MultiServiceRouter,
    kind: Option<&str>,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut query = Vec::new();
    if let Some(kind) = kind {
        query.push(format!("type={kind}"));
    }
    if let Some(max) = max_results {
        query.push(format!("maxResults={max}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let uri = if query.is_empty() {
        "/browsers".to_string()
    } else {
        format!("/browsers?{}", query.join("&"))
    };
    send(router, "POST", &uri, Some(&json!({}))).await
}

/// `CreateBrowser`: `PUT /browsers`, answering HTTP 202. Hands back the `browserId` the
/// create minted.
async fn create_browser(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = send(
        router,
        "PUT",
        "/browsers",
        Some(&json!({
            "name": name,
            "networkConfiguration": {"networkMode": "PUBLIC"},
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {name:?}: expected HTTP 202 from CreateBrowser, got {status} ({created})"
    );
    created
        .get("browserId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {name:?}: the create reported no browserId ({created})"))
        .to_string()
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `browserId`s a listing reported, in the order it reported them.
fn listed_ids(body: &Value) -> Vec<String> {
    body["browserSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| {
                    summary["browserId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Create `count` browsers under `prefix` and hand back every `browserId` the account
/// then holds — the ones it created and, second, the service's own tool beside them — each
/// in the ascending order a listing reports them.
///
/// `SandboxName` is `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`, so the names built here are inside it.
async fn seed(
    router: &MultiServiceRouter,
    prefix: &str,
    count: usize,
) -> (Vec<String>, Vec<String>) {
    let mut custom = Vec::new();
    for index in 1..=count {
        custom.push(create_browser(router, &format!("{prefix}{index:02}")).await);
    }
    custom.sort();
    let mut everything = custom.clone();
    everything.push(SYSTEM_BROWSER_ID.to_string());
    everything.sort();
    (custom, everything)
}

/// The account most of these tests page over: [`CREATED_BROWSERS`] browsers of its own
/// beside the service's tool.
async fn seeded(router: &MultiServiceRouter) -> (Vec<String>, Vec<String>) {
    seed(router, "pagedBrowser", CREATED_BROWSERS).await
}

/// Walk the listing from its first page to its last, following the `nextToken` each page
/// hands back, and report the ids in the order the pages reported them together with how
/// many pages the walk took. `held` is how many browsers the account holds, which bounds
/// how many pages even the smallest page size can take.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    kind: Option<&str>,
    max_results: Option<i64>,
    held: usize,
) -> (Vec<String>, usize) {
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        let (status, page) = list_browsers(router, kind, max_results, token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?} (page {pages}): expected HTTP 200 from ListBrowsers, got {status} \
             ({page})"
        );
        let ids = listed_ids(&page);
        let allowed = max_results
            .map_or(DOCUMENTED_DEFAULT_PAGE, |max| {
                usize::try_from(max).unwrap_or(DOCUMENTED_MAX_PAGE)
            })
            .min(DOCUMENTED_MAX_PAGE);
        assert!(
            ids.len() <= allowed,
            "case {case:?} (page {pages}): a page holds at most {allowed} summaries, got {} \
             ({page})",
            ids.len()
        );
        walked.extend(ids);
        pages += 1;
        // The smallest page the model admits is one entry, so even that walk cannot take
        // more pages than the account holds browsers, plus the empty page a cursor sitting
        // exactly on the last of them would answer.
        assert!(
            pages <= held + 2,
            "case {case:?}: the walk must terminate, got {pages} pages and {walked:?}"
        );
        match page["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    (walked, pages)
}

/// A listing the caller gave no `maxResults` holds the page size that member's
/// documentation states — not every browser the account holds — and the token it hands
/// back resumes on the rest.
#[tokio::test]
async fn a_listing_with_no_max_results_holds_the_documented_default_page() {
    let router = router();
    let (_, everything) = seeded(&router).await;
    assert!(
        everything.len() > DOCUMENTED_DEFAULT_PAGE,
        "case \"the seeded account\": the account must hold more than one default page for \
         this rule to be observable, got {}",
        everything.len()
    );

    let (status, first) = list_browsers(&router, None, None, None).await;
    assert_eq!(
        status, 200,
        "case \"the first unpaged page\": expected HTTP 200, got {status} ({first})"
    );
    assert_eq!(
        listed_ids(&first),
        everything[..DOCUMENTED_DEFAULT_PAGE].to_vec(),
        "case \"the first unpaged page\": expected the documented default of \
         {DOCUMENTED_DEFAULT_PAGE} summaries in ascending browserId order, got {first}"
    );
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case \"the first unpaged page\": {} browsers are left over, so the page hands \
                 back a token, got {first}",
                everything.len() - DOCUMENTED_DEFAULT_PAGE
            )
        })
        .to_string();

    let (status, second) = list_browsers(&router, None, None, Some(&token)).await;
    assert_eq!(
        status, 200,
        "case \"the page the token resumes\": expected HTTP 200, got {status} ({second})"
    );
    assert_eq!(
        listed_ids(&second),
        everything[DOCUMENTED_DEFAULT_PAGE..].to_vec(),
        "case \"the page the token resumes\": expected the browsers past the first page, got \
         {second}"
    );
    assert!(
        second["nextToken"].is_null(),
        "case \"the page the token resumes\": nothing is left over past the last browser, got {}",
        second["nextToken"]
    );
}

/// For every page size the modelled range admits, the pages concatenate to the whole
/// listing: each browser once, in ascending `browserId` order, with no page holding more
/// than `maxResults` summaries.
///
/// The rows differ along the page size, which is what paging generalises over: one entry
/// per page, sizes that leave a part-full last page, the size that ends exactly on the
/// last browser, one past the number held, and each modelled bound.
#[tokio::test]
async fn pages_of_every_size_concatenate_to_the_whole_listing() {
    let router = router();
    let (_, everything) = seeded(&router).await;
    let held = i64::try_from(everything.len()).unwrap_or(i64::MAX);

    // (case, maxResults)
    let rows: [(&str, Option<i64>); 8] = [
        ("the modelled minimum, one browser per page", Some(MAX_RESULTS_MIN)),
        ("two browsers per page", Some(2)),
        ("a size leaving a part-full last page", Some(5)),
        ("the documented default, named explicitly", Some(10)),
        ("exactly as many as the account holds", Some(held)),
        ("one more than the account holds", Some(held + 1)),
        ("the modelled maximum", Some(MAX_RESULTS_MAX)),
        ("no maxResults at all", None),
    ];

    for (case, max_results) in rows {
        let (walked, pages) = walk(&router, case, None, max_results, everything.len()).await;
        assert_eq!(
            walked, everything,
            "case {case:?}: the pages must concatenate to the whole listing {everything:?}, got \
             {walked:?}"
        );
        assert!(
            pages >= 1,
            "case {case:?}: a listing is served in at least one page, got {pages}"
        );
    }
}

/// No page holds more than the maximum `maxResults`' documentation states, however large a
/// page the caller asks for — and a `maxResults` past that documented maximum but inside
/// the modelled range is still ACCEPTED, because the range is a constraint on the value,
/// not a promise about the page.
///
/// The rows differ along the page size asked for: the documented maximum itself, one past
/// it, a size between it and the modelled maximum, and the modelled maximum.
#[tokio::test]
async fn no_page_holds_more_than_the_documented_maximum() {
    let router = router();
    // One more browser than the documented maximum, so a page capped at it has something
    // left over to hand a token back for.
    let (_, everything) = seed(&router, "cappedBrowser", DOCUMENTED_MAX_PAGE + 1).await;
    assert_eq!(
        everything.len(),
        DOCUMENTED_MAX_PAGE + 2,
        "case \"the seeded account\": expected the created browsers and the service's own, got \
         {everything:?}"
    );

    // (case, maxResults)
    let rows: [(&str, Option<i64>); 5] = [
        ("the documented maximum", Some(50)),
        ("one past the documented maximum", Some(51)),
        ("between the documented and the modelled maximum", Some(75)),
        ("the modelled maximum", Some(MAX_RESULTS_MAX)),
        ("no maxResults at all", None),
    ];

    for (case, max_results) in rows {
        let expected_page = max_results
            .map_or(DOCUMENTED_DEFAULT_PAGE, |max| {
                usize::try_from(max).unwrap_or(DOCUMENTED_MAX_PAGE)
            })
            .min(DOCUMENTED_MAX_PAGE);
        let (status, page) = list_browsers(&router, None, max_results, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListBrowsers, got {status} ({page})"
        );
        assert_eq!(
            listed_ids(&page),
            everything[..expected_page].to_vec(),
            "case {case:?}: expected the first {expected_page} browsers of the listing, got {page}"
        );
        assert!(
            page["nextToken"].as_str().is_some(),
            "case {case:?}: browsers are left over past a page of {expected_page}, so the page \
             hands back a token, got {}",
            page["nextToken"]
        );

        // And the capped pages still account for every browser exactly once.
        let (walked, pages) = walk(&router, case, None, max_results, everything.len()).await;
        assert_eq!(
            walked, everything,
            "case {case:?}: the pages must concatenate to the whole listing, got {walked:?}"
        );
        assert!(
            pages >= 2,
            "case {case:?}: a listing longer than one capped page takes more than one page, got \
             {pages}"
        );
    }
}

/// One row of the filtered-paging table: the case, the `type` the listing filters by, the
/// `maxResults` each page asks for, and the `browserId`s the walk must report.
type FilteredRow = (
    &'static str,
    Option<&'static str>,
    Option<i64>,
    Vec<String>,
);

/// The `type` filter narrows what a page holds, not how big a page is: paging over the
/// browsers this account created reports exactly those, and the service's own tool is the
/// whole of the `SYSTEM` listing however small the page.
#[tokio::test]
async fn paging_a_filtered_listing_reports_exactly_that_type() {
    let router = router();
    let (custom, everything) = seeded(&router).await;

    // (case, the type filter, maxResults, the ids the walk must report)
    let rows: Vec<FilteredRow> = vec![
        ("what this account created, one per page", Some("CUSTOM"), Some(1), custom.clone()),
        ("what this account created, five per page", Some("CUSTOM"), Some(5), custom.clone()),
        ("what this account created, the default page", Some("CUSTOM"), None, custom.clone()),
        (
            "what the service provides, one per page",
            Some("SYSTEM"),
            Some(1),
            vec![SYSTEM_BROWSER_ID.to_string()],
        ),
        (
            "what the service provides, the default page",
            Some("SYSTEM"),
            None,
            vec![SYSTEM_BROWSER_ID.to_string()],
        ),
        ("every type, five per page", None, Some(5), everything.clone()),
    ];

    for (case, kind, max_results, expected) in rows {
        let (walked, _) = walk(&router, case, kind, max_results, everything.len()).await;
        assert_eq!(
            walked, expected,
            "case {case:?}: expected the walk to report {expected:?}, got {walked:?}"
        );
    }

    // A single `SYSTEM` page leaves nothing over, so it carries no token at all.
    let (status, listed) = list_browsers(&router, Some("SYSTEM"), Some(MAX_RESULTS_MIN), None).await;
    assert_eq!(
        (status, listed["nextToken"].clone()),
        (200, Value::Null),
        "case \"the only SYSTEM browser\": a page holding every match carries no nextToken, got \
         {listed}"
    );
}

/// `maxResults` outside its modelled range (1..100) is refused at the boundary with the
/// modelled 400, and each modelled bound itself is served.
#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let router = router();
    let (_, everything) = seeded(&router).await;

    // (case, maxResults, expected status, how many summaries a served page holds)
    let rows: [(&str, i64, u16, usize); 4] = [
        ("one below the modelled minimum", MAX_RESULTS_MIN - 1, 400, 0),
        (
            "the modelled minimum",
            MAX_RESULTS_MIN,
            200,
            usize::try_from(MAX_RESULTS_MIN).unwrap_or(1),
        ),
        ("the modelled maximum", MAX_RESULTS_MAX, 200, everything.len()),
        ("one above the modelled maximum", MAX_RESULTS_MAX + 1, 400, 0),
    ];

    for (case, max_results, expected_status, expected_len) in rows {
        let (status, answered) = list_browsers(&router, None, Some(max_results), None).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for maxResults={max_results}, got \
             {status} ({answered})"
        );
        if expected_status == 200 {
            assert_eq!(
                listed_ids(&answered),
                everything[..expected_len].to_vec(),
                "case {case:?}: expected the first {expected_len} browsers held, got {answered}"
            );
        } else {
            assert_eq!(
                error_code(&answered),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {answered}"
            );
        }
    }
}

/// A `nextToken` this service did not issue is the modelled 400 naming that member, while
/// one it could have issued is served — walking off the end of the store is an empty page,
/// not a refusal.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = router();
    let (_, everything) = seeded(&router).await;

    for (case, token) in [
        ("a word outside the token's alphabet", "notatoken"),
        ("a pair of characters outside it", "zz"),
        ("a token truncated to an odd length", "616"),
        ("a browserId handed over unencoded", "pagedBrowser01"),
    ] {
        let (status, answered) = list_browsers(&router, None, None, Some(token)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for nextToken={token:?}, got {status} ({answered})"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {answered}"
        );
        assert_eq!(
            answered["fieldList"][0]["name"],
            json!("nextToken"),
            "case {case:?}: expected the refusal to name nextToken, got {}",
            answered["fieldList"]
        );
    }

    // `7a7a7a7a` is the token form this service issues (the hex of the store key a page
    // ended on) for the key `zzzz`, which sorts after every browserId the account holds —
    // so it resumes past the last of them.
    let (status, beyond) = list_browsers(&router, None, None, Some("7a7a7a7a")).await;
    assert_eq!(
        (status, listed_ids(&beyond)),
        (200, Vec::<String>::new()),
        "case \"a token that resumes past every browser\": expected the required \
         browserSummaries present and empty, got {beyond}"
    );
    assert!(
        beyond["nextToken"].is_null(),
        "case \"a token that resumes past every browser\": nothing is left over, got {}",
        beyond["nextToken"]
    );

    // And the token a page DID hand back resumes strictly after the entry it was issued
    // for: no browser is reported twice across the two pages.
    let (status, first) = list_browsers(&router, None, Some(MAX_RESULTS_MIN), None).await;
    assert_eq!(
        status, 200,
        "case \"the first single-entry page\": expected HTTP 200, got {status} ({first})"
    );
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"the first single-entry page\": expected a token ({first})"))
        .to_string();
    let (status, second) = list_browsers(&router, None, Some(MAX_RESULTS_MIN), Some(&token)).await;
    assert_eq!(
        (status, listed_ids(&second)),
        (200, vec![everything[1].clone()]),
        "case \"the page the token resumes\": expected the browser after {}, got {second}",
        everything[0]
    );
}
