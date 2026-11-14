// SPDX-License-Identifier: CC0-1.0

//! What this service's paginated reads answer when the request names no
//! `maxResults`.
//!
//! Every assertion here is about ONE rule: a paginated read answers a PAGE —
//! the size its `maxResults` member documents a default for, and, where the
//! member documents no default, the largest size that member's modelled range
//! admits — and carries a `nextToken` exactly when entries remain past that
//! page. The tables span what the rule generalises over: how the stored count
//! sits against the page size (far under it, one under, exactly it, one over,
//! far over), which read answers, and whether a continuing request names a page
//! size or none.

use serde_json::{json, Value};

use crate::billing_views::{call, emulator, ACCOUNT_ID, PRIMARY_ARN};

/// The page size `ListBillingViews` serves a request that names none.
///
/// From the `maxResults` member doc of `ListBillingViewsRequest`: "The maximum
/// number of billing views to retrieve. Default is 100."
const LIST_BILLING_VIEWS_DEFAULT_PAGE: usize = 100;

/// The largest page `ListSourceViewsForBillingView` serves, and so the size it
/// answers a request that names none: its `maxResults` member documents no
/// default ("The number of entries a paginated response contains."), and
/// `BillingViewsMaxResults` is modelled `@range(min: 1, max: 100)`.
const SOURCE_VIEWS_MAX_PAGE: usize = 100;

/// The modelled `@length` maximum of `BillingViewSourceViewsList`, the most
/// source views one billing view can draw from.
const MAX_SOURCE_VIEWS: usize = 10;

/// A ceiling on the pages one walk below may take, so a walk that fails to make
/// progress fails the test instead of running forever.
const MAX_PAGES: usize = 64;

/// Create `count` custom billing views and answer their ARNs.
async fn create_views(
    router: &smithy_server_runtime::MultiServiceRouter,
    count: usize,
    case: &str,
) -> Vec<String> {
    let mut arns = Vec::with_capacity(count);
    for index in 0..count {
        let body = json!({
            "name": format!("page-size-{index}"),
            "sourceViews": [PRIMARY_ARN],
            "clientToken": format!("page-size-{index}"),
        });
        let (status, created) = call(router, "CreateBillingView", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected view {index} to be created, got {status} {created:?}"
        );
        arns.push(
            created["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a create answers an arn, got {created:?}"))
                .to_string(),
        );
    }
    arns
}

/// The ARNs a `ListBillingViews` answer reported, in the order it reported them.
fn listed_arns(body: &Value) -> Vec<String> {
    body["billingViews"]
        .as_array()
        .unwrap_or_else(|| panic!("billingViews is a list, got {body:?}"))
        .iter()
        .map(|view| view["arn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every billing view ARN the account holds after `created`, in the order the
/// ARN-ordered read reports them.
fn every_arn(created: &[String]) -> Vec<String> {
    let mut all: Vec<String> = created.to_vec();
    all.push(PRIMARY_ARN.to_string());
    all.sort();
    all
}

#[tokio::test]
async fn list_billing_views_with_no_max_results_answers_one_default_page() {
    // The rows differ in how the number of views the account holds sits against
    // the documented default page size: far under it, one under, exactly it,
    // one over, and half a page over. The PRIMARY view every account owns is
    // part of the result set, so the held count is one more than the created
    // count.
    for created_count in [0_usize, 4, 98, 99, 100, 149] {
        let case = format!("{created_count} created views, no maxResults");
        let router = emulator();
        let created = create_views(&router, created_count, &case).await;
        let expected_all = every_arn(&created);
        let expected_page = expected_all.len().min(LIST_BILLING_VIEWS_DEFAULT_PAGE);

        let (status, listed) = call(&router, "ListBillingViews", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListBillingViews, got {status} {listed:?}"
        );
        let page = listed_arns(&listed);
        assert_eq!(
            page.len(),
            expected_page,
            "case {case:?}: expected a page of {expected_page} of {} views, got {} ({page:?})",
            expected_all.len(),
            page.len()
        );
        assert_eq!(
            page.as_slice(),
            &expected_all[..expected_page],
            "case {case:?}: expected the first {expected_page} views in ARN order, got {page:?}"
        );
        let token = listed["nextToken"].as_str();
        assert_eq!(
            token.is_some(),
            expected_all.len() > expected_page,
            "case {case:?}: a nextToken is carried exactly when views remain past the page, got {token:?}"
        );
    }
}

#[tokio::test]
async fn a_walk_that_names_no_max_results_reports_every_view_exactly_once() {
    // The rows differ in how many pages of the default size the result set
    // spans, and in whether the continuing request names a page size (a smaller
    // one than the default) or none at all: the pages concatenate to the whole
    // ARN-ordered set either way, with nothing skipped and nothing repeated.
    let rows: &[(&str, usize, Option<usize>)] = &[
        ("one page, continued with no maxResults", 50, None),
        ("two pages, continued with no maxResults", 149, None),
        ("two pages, continued with maxResults=7", 149, Some(7)),
        ("two pages, continued with maxResults=100", 101, Some(100)),
    ];
    for &(case, created_count, resume_page_size) in rows {
        let router = emulator();
        let created = create_views(&router, created_count, case).await;
        let expected_all = every_arn(&created);

        let mut seen: Vec<String> = Vec::new();
        let mut request = json!({});
        for taken in 0..MAX_PAGES {
            let (status, listed) = call(&router, "ListBillingViews", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from ListBillingViews, got {status} {listed:?}"
            );
            let page = listed_arns(&listed);
            let cap = match resume_page_size {
                Some(size) if taken > 0 => size,
                _ => LIST_BILLING_VIEWS_DEFAULT_PAGE,
            };
            assert!(
                page.len() <= cap,
                "case {case:?}: page {taken} must not exceed {cap} entries, got {} ({page:?})",
                page.len()
            );
            seen.extend(page);
            match listed["nextToken"].as_str() {
                Some(token) => {
                    request = match resume_page_size {
                        Some(size) => json!({ "nextToken": token, "maxResults": size }),
                        None => json!({ "nextToken": token }),
                    };
                }
                None => break,
            }
            assert!(
                taken + 1 < MAX_PAGES,
                "case {case:?}: the walk must terminate, got {MAX_PAGES} pages of {seen:?}"
            );
        }
        assert_eq!(
            seen, expected_all,
            "case {case:?}: expected every view exactly once in ARN order, got {} entries",
            seen.len()
        );
    }
}

#[tokio::test]
async fn list_source_views_with_no_max_results_answers_a_bounded_page() {
    // The rows are the source-view counts a view can be created with, from one
    // to the modelled maximum: each is below the largest page this read serves,
    // so a request naming no `maxResults` reports them all on one page and
    // carries no token — and a request naming a size of one still walks them.
    for source_count in 1..=MAX_SOURCE_VIEWS {
        let case = format!("{source_count} source views, no maxResults");
        let router = emulator();
        let sources: Vec<String> = std::iter::once(PRIMARY_ARN.to_string())
            .chain(
                (1..source_count)
                    .map(|index| format!("arn:aws:billing::{ACCOUNT_ID}:billingview/source-{index}")),
            )
            .collect();
        let (status, created) = call(
            &router,
            "CreateBillingView",
            &json!({
                "name": "sources",
                "sourceViews": sources,
                "clientToken": "sources",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the view to be created, got {status} {created:?}"
        );
        let arn = created["arn"].as_str().expect("a create answers an arn");

        let (status, listed) = call(
            &router,
            "ListSourceViewsForBillingView",
            &json!({ "arn": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListSourceViewsForBillingView, got {status} {listed:?}"
        );
        let page: Vec<String> = listed["sourceViews"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: sourceViews is a list, got {listed:?}"))
            .iter()
            .map(|entry| entry.as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            page.len() <= SOURCE_VIEWS_MAX_PAGE,
            "case {case:?}: a page must not exceed {SOURCE_VIEWS_MAX_PAGE} entries, got {} ({page:?})",
            page.len()
        );
        assert_eq!(
            page, sources,
            "case {case:?}: expected every stored source view on one page, got {page:?}"
        );
        let token = listed["nextToken"].as_str();
        assert!(
            token.is_none(),
            "case {case:?}: no entries remain past the page, so no nextToken is carried, got {token:?}"
        );

        // The same set, one entry to a page: the walk reports exactly the
        // stored list, in the order the view holds it.
        let mut seen: Vec<String> = Vec::new();
        let mut request = json!({ "arn": arn, "maxResults": 1 });
        for taken in 0..MAX_PAGES {
            let (status, listed) = call(&router, "ListSourceViewsForBillingView", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 on page {taken}, got {status} {listed:?}"
            );
            let page: Vec<String> = listed["sourceViews"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: sourceViews is a list, got {listed:?}"))
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                page.len() <= 1,
                "case {case:?}: page {taken} must not exceed maxResults=1, got {page:?}"
            );
            seen.extend(page);
            match listed["nextToken"].as_str() {
                Some(token) => request["nextToken"] = json!(token),
                None => break,
            }
            assert!(
                taken + 1 < MAX_PAGES,
                "case {case:?}: the walk must terminate, got {MAX_PAGES} pages of {seen:?}"
            );
        }
        assert_eq!(
            seen, sources,
            "case {case:?}: expected every stored source view exactly once, got {seen:?}"
        );
    }
}
