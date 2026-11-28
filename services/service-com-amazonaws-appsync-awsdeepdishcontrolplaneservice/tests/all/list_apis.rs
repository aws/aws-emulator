// SPDX-License-Identifier: CC0-1.0

//! `ListApis`, end to end over HTTP: what a page holds, what resumes it, and which
//! listings are refused.
//!
//! The rule is stated over CLASSES of input rather than over the one request that
//! exposed the defect: a listing reports every `Api` this account holds — and only an
//! `Api`, never a GraphQL API, which is a resource of its own — in one order that does
//! not depend on how the caller pages, cut into a page of at most `maxResults` entries
//! (the modelled maximum when the request names none) and resumed by the `nextToken`
//! the previous page handed back, which is reported exactly while entries remain; each
//! entry is the record `GetApi` answers for that id; and the call is a pure read, so
//! paging leaves every later reader seeing the same set.
//!
//! The rows span:
//!
//! 1. the count of APIs the account holds — none, one, several, a count that pages
//!    several times over, and the whole of what an account may hold — so neither the
//!    page nor its cursor can be keyed to a single collection size;
//! 2. the `maxResults` the caller names — unspecified, and each of several values
//!    inside the modelled range, including its published bounds;
//! 3. the authorization configuration of the listed APIs, rotated across every
//!    `AuthenticationType` the model declares, so the projection cannot be keyed to one
//!    kind of API;
//! 4. the forms of a listing that cannot resume: tokens that name no page of the list,
//!    and a `maxResults` one past each published bound of its modelled range.
//!
//! The harness (`call`, `create_api`, `router`, `Answer`) is the error-contract suite's;
//! the Event API body builder and the authorization-type catalog are the Event API
//! suite's — reused rather than copied.

use serde_json::Value;
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, create_api, router, Answer};
use crate::event_api::{create_event_api, event_config, AUTHENTICATION_TYPES};

/// The counts of `Api` resources an account is listed at: none, one, several, a count
/// that pages several times over at the smaller page sizes, and the whole of what an
/// account may hold — AppSync's per-Region API quota, which `CreateApi` refuses past,
/// so no account can hold more than this.
const COUNTS: &[usize] = &[0, 1, 4, 13, state::DEFAULT_PAGE_SIZE];

/// The `maxResults` values a walk pages with: unspecified, the modelled default sent
/// explicitly, and values inside the modelled range up to its published maximum.
/// Source: the `maxResults` member of `ListApisRequest` (`@range` 0..25, default 0 —
/// i.e. unspecified, so `0` and an absent member select the same page).
const PAGE_SIZES: &[Option<i32>] = &[None, Some(0), Some(1), Some(2), Some(7), Some(25)];

/// The half of the text for a `nextToken` a listing cannot resume from that is fixed: the
/// sentence also carries the token decoder's diagnosis of the payload.
const INVALID_TOKEN: &str = "Pagination token exception in operation 'ListApis': ";

/// One page of a listing: the ids it reported, the cursor that resumes after them, and
/// the entries themselves.
#[derive(Debug)]
struct ListedPage {
    ids: Vec<String>,
    next_token: Option<String>,
    apis: Vec<Value>,
}

/// The name the Event API at `index` of a seeded account carries.
fn seeded_name(index: usize) -> String {
    format!("Events_{index:04}")
}

/// Create `count` Event APIs, their authorization configurations rotating across every
/// `AuthenticationType` the model declares, and return their `apiId`s in creation
/// order.
async fn seed(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let auth_type = AUTHENTICATION_TYPES[index % AUTHENTICATION_TYPES.len()];
        let name = seeded_name(index);
        let created = create_event_api(
            router,
            serde_json::json!({
                "name": name,
                "eventConfig": event_config(
                    &[auth_type], &[auth_type], &[auth_type], &[auth_type], None,
                ),
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case seed {name:?} authorized by {auth_type}: expected 200, got {created:?}"
        );
        ids.push(
            created.body["api"]["apiId"]
                .as_str()
                .unwrap_or_else(|| panic!("case seed {name:?}: expected an apiId, got {created:?}"))
                .to_string(),
        );
    }
    ids
}

/// Send one `ListApis`, with the `maxResults` and `nextToken` the caller names.
async fn list_page(
    router: &MultiServiceRouter,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(next_token) = next_token {
        query.push(format!("nextToken={next_token}"));
    }
    let path = if query.is_empty() {
        "/v2/apis".to_string()
    } else {
        format!("/v2/apis?{}", query.join("&"))
    };
    call(router, "GET", &path, None).await
}

/// One page as the suite reads it, asserting it is a 200 that reports the list member.
fn read_page(case: &str, answer: &Answer) -> ListedPage {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected ListApis 200, got {answer:?}"
    );
    let apis: Vec<Value> = answer.body["apis"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an apis list, got {answer:?}"))
        .clone();
    let ids = apis
        .iter()
        .map(|api| {
            api["apiId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected every entry to carry an apiId, got {answer:?}")
                })
                .to_string()
        })
        .collect();
    ListedPage {
        ids,
        next_token: answer.body["nextToken"].as_str().map(str::to_string),
        apis,
    }
}

/// Page through the whole account at `max_results` and return every entry, in the order
/// the listing reported them, asserting the page-size bound and the cursor contract on
/// each page.
async fn walk(
    router: &MultiServiceRouter,
    max_results: Option<i32>,
    expected_total: usize,
) -> Vec<Value> {
    // An unspecified `maxResults` — absent, or the modelled default of `0` — selects
    // the whole modelled page.
    let bound = match max_results {
        Some(size) if size > 0 => size as usize,
        _ => state::DEFAULT_PAGE_SIZE,
    };
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // A page carries at least one entry while entries remain, so the walk cannot take
    // more pages than there are entries, plus the single page of an empty listing.
    for page_index in 0..expected_total + 1 {
        let case = format!("walk at maxResults {max_results:?}, page {page_index} from {token:?}");
        let answer = list_page(router, max_results, token.as_deref()).await;
        let page = read_page(&case, &answer);
        let remaining = expected_total - collected.len();
        assert_eq!(
            page.ids.len(),
            remaining.min(bound),
            "case {case}: expected the page to be as full as the remainder allows (at most \
             {bound}), got {:?}",
            page.ids
        );
        collected.extend(page.apis);
        // The cursor is reported exactly while entries remain after this page.
        let more = collected.len() < expected_total;
        assert_eq!(
            page.next_token.is_some(),
            more,
            "case {case}: expected a nextToken {}, got {:?}",
            if more { "while entries remain" } else { "on the last page" },
            page.next_token
        );
        match page.next_token {
            None => return collected,
            Some(cursor) => token = Some(cursor),
        }
    }
    panic!("case walk at maxResults {max_results:?}: the walk never reached a last page");
}

/// The `apiId` of every entry of a listing, in the order it reported them.
fn ids(apis: &[Value]) -> Vec<String> {
    apis.iter()
        .map(|api| api["apiId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every walk of an account reports the same APIs in the same order, however the caller
/// pages, and every API it holds exactly once — in the order they were created.
#[tokio::test]
async fn every_page_size_walks_the_whole_account_in_one_order() {
    for count in COUNTS.iter().copied() {
        // `ListApis` reports the whole account, so each count is seeded on its own
        // engine rather than on a shared one.
        let router = router();
        let seeded = seed(&router, count).await;

        for max_results in PAGE_SIZES.iter().copied() {
            let listed = walk(&router, max_results, count).await;
            let listed_ids = ids(&listed);
            assert_eq!(
                listed_ids, seeded,
                "case {count} APIs at maxResults {max_results:?}: expected every seeded API \
                 exactly once, in creation order {seeded:?}, got {listed_ids:?}"
            );
        }
    }
}

/// Each listed entry is the record `GetApi` answers for that id, member for member — so
/// a listing projects the stored API rather than a summary of it.
#[tokio::test]
async fn a_listed_entry_is_the_record_get_answers() {
    let router = router();
    // One API per `AuthenticationType`, so the projection is asserted on every kind of
    // authorization configuration the model declares.
    let count = AUTHENTICATION_TYPES.len();
    seed(&router, count).await;

    for max_results in PAGE_SIZES.iter().copied() {
        let listed = walk(&router, max_results, count).await;
        for entry in &listed {
            let api_id = entry["apiId"].as_str().unwrap_or_default();
            let read = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
            assert_eq!(
                read.status, 200,
                "case listed {api_id:?} at maxResults {max_results:?}: expected GetApi 200, got \
                 {read:?}"
            );
            assert_eq!(
                &read.body["api"], entry,
                "case listed {api_id:?} at maxResults {max_results:?}: expected the listing to \
                 answer the stored record {:?}, got {entry:?}",
                read.body["api"]
            );
        }
    }
}

/// An `Api` is a resource of its own: a GraphQL API is created by another operation and
/// never appears in this listing, however many of either the account holds.
#[tokio::test]
async fn a_graphql_api_is_not_listed_as_an_api() {
    // The rows differ along how many APIs of each family the account holds, including
    // the two edges where one family is empty.
    for (event_apis, graphql_apis) in [(0usize, 3usize), (3, 0), (2, 5), (6, 1)] {
        let router = router();
        let seeded = seed(&router, event_apis).await;
        let mut graphql_ids = Vec::with_capacity(graphql_apis);
        for index in 0..graphql_apis {
            graphql_ids.push(create_api(&router, &format!("Api_graphql_{index:04}")).await);
        }

        let case = format!("{event_apis} Event APIs beside {graphql_apis} GraphQL APIs");
        let listed = ids(&walk(&router, Some(1), event_apis).await);
        assert_eq!(
            listed, seeded,
            "case {case}: expected only the Event APIs {seeded:?}, got {listed:?}"
        );
        for graphql_id in &graphql_ids {
            assert!(
                !listed.contains(graphql_id),
                "case {case}: expected the GraphQL API {graphql_id:?} to be absent, got {listed:?}"
            );
        }
    }
}

/// Listing is a pure read: paging through the account changes nothing a later caller
/// sees, so the walk repeats identically and every entry still reads back.
#[tokio::test]
async fn paging_leaves_the_stored_set_unchanged() {
    let router = router();
    let count = state::DEFAULT_PAGE_SIZE;
    let seeded = seed(&router, count).await;

    // A first walk at the smallest page, so the listing is paged all the way through,
    // then the same walk again and one at the modelled maximum.
    let first = walk(&router, Some(1), count).await;
    let again = walk(&router, Some(1), count).await;
    let whole = walk(&router, Some(25), count).await;

    assert_eq!(
        again, first,
        "case a repeated walk: expected the same entries as the first walk {first:?}, got \
         {again:?}"
    );
    assert_eq!(
        whole, first,
        "case a walk at the modelled maximum after paging: expected the same entries {first:?}, \
         got {whole:?}"
    );
    for api_id in &seeded {
        let read = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {api_id:?} after paging: expected GetApi 200, got {read:?}"
        );
    }
}

/// A listing that cannot resume is refused: a `nextToken` that names no page of the
/// list takes AppSync's fixed invalid-token text, and a `maxResults` past either
/// published bound of its modelled range is refused naming the member and the range —
/// while both published bounds, and a token the listing itself handed back, are served.
#[tokio::test]
async fn a_listing_that_cannot_resume_is_refused() {
    let router = router();
    let seeded = seed(&router, 3).await;

    // Tokens that name no page of this list: an id-shaped value nothing created, a
    // name rather than an id, and values of other shapes entirely.
    for token in [
        "bogus",
        "0",
        &"z".repeat(state::API_ID_LEN),
        seeded_name(0).as_str(),
        "da2-nosuchkey",
    ] {
        for max_results in PAGE_SIZES.iter().copied() {
            let answer = list_page(&router, max_results, Some(token)).await;
            assert_eq!(
                answer.status, 400,
                "case resuming from {token:?} at maxResults {max_results:?}: expected status 400, \
                 got {answer:?}"
            );
            assert_eq!(
                answer.error_type, "BadRequestException",
                "case resuming from {token:?} at maxResults {max_results:?}: expected \
                 BadRequestException, got {answer:?}"
            );
            assert!(
                answer.message.starts_with(INVALID_TOKEN),
                "case resuming from {token:?} at maxResults {max_results:?}: expected the \
                 refusal to name the operation the token was submitted to, got {answer:?}"
            );
        }
    }

    // One row stated in FULL, so this suite pins the whole sentence and not only the half
    // that names the operation: `bogus`'s payload states the wrapping schema 28296 (the
    // leading 16 bits of the bytes its base64 characters `b`, `o`, `g` spell).
    let answer = list_page(&router, None, Some("bogus")).await;
    assert_eq!(
        answer.message,
        format!("{INVALID_TOKEN}invalid payload wrapping schema 28296"),
        "case resuming from \"bogus\": expected the whole sentence, got {answer:?}"
    );

    // `maxResults` is modelled `@range` 0..25: both published bounds are served, and one
    // past either bound is refused with the validation layer's own constraint report, which
    // is the answer THIS surface's recordings show for the member (the whole report, and
    // the v1 control plane's own sentence for the same member, are asserted in
    // `crate::aws_worded_refusals`).
    for (max_results, expected) in [(0, 200), (25, 200), (26, 400), (-1, 400)] {
        let answer = list_page(&router, Some(max_results), None).await;
        assert_eq!(
            answer.status, expected,
            "case maxResults {max_results}: expected status {expected}, got {answer:?}"
        );
        if expected == 400 {
            assert_eq!(
                answer.error_type, "ValidationException",
                "case maxResults {max_results}: expected ValidationException, got {answer:?}"
            );
            let bound = if max_results < 0 {
                "greater than or equal to 0"
            } else {
                "less than or equal to 25"
            };
            assert!(
                answer.message.contains("Value at 'maxResults'")
                    && answer.message.contains(bound),
                "case maxResults {max_results}: expected the report to name the member and the \
                 bound {bound:?} it crossed, got {answer:?}"
            );
        }
    }

    // A token the listing itself handed back still resumes, so the refusals above are
    // about tokens that name no page rather than about paging at all.
    let first = list_page(&router, Some(1), None).await;
    let page = read_page("the first page of the refusal account", &first);
    let cursor = page
        .next_token
        .expect("the first page of three APIs carries a nextToken");
    let resumed = list_page(&router, Some(1), Some(&cursor)).await;
    let resumed = read_page("resuming from the listing's own token", &resumed);
    assert_eq!(
        resumed.ids,
        vec![seeded[1].clone()],
        "case resuming from the listing's own token: expected the next API, got {:?}",
        resumed.ids
    );
}
