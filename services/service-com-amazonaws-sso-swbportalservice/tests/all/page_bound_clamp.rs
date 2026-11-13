// SPDX-License-Identifier: CC0-1.0

//! The page bound every portal listing shares: a bound the caller NAMED is carried into
//! the page-size range the model allows, and only the ABSENCE of a bound takes the
//! default page.
//!
//! The rule under test belongs to the one page-cutter `src/state.rs` shares, so it holds
//! for every paginated operation of this service alike: a named bound below the range is
//! served at the range's floor, a named bound above it at the range's ceiling, and a
//! request that names no bound at all — the only case there is no bound to carry — is
//! served the default page. A named bound is therefore never answered as if the caller
//! had named none.
//!
//! The rows differ along everything the rule generalises over: whether a bound was named,
//! how far outside the range it falls (each published bound, one past each, and each end
//! of the `i32` domain the member is modeled over), how it sits relative to the number of
//! items available, and which listing is being cut (the session's accounts, one account's
//! roles). The range itself comes from the model's `MaxResultType` (range 1..100), read
//! with `smithy_query(query="shape",shape="MaxResultType")`.
//!
//! Values outside that range are refused at the boundary before a handler runs, which
//! `portal_session.rs` pins, so those rows exercise the shared cutter through the public
//! `state` API — the same surface the suites seed the store through — while the in-range
//! rows go over HTTP through both listings.

use http::StatusCode;
use service_com_amazonaws_sso_swbportalservice::state::{self, MAX_PAGE_SIZE};

use crate::portal_session::{body_json, body_text, Portal};

/// How many items every listing below holds: more than the largest page the service
/// serves, so a row asking for the ceiling (or more) is still a cut listing and the
/// difference between the ceiling and the floor is visible.
const TOTAL: usize = MAX_PAGE_SIZE + 5;

/// The sign-in session the HTTP rows read with.
const ACCESS_TOKEN: &str = "tok-page-bound";

/// One `maxResults` a caller may hand the shared cutter, and the page size the rule says
/// it is served at.
struct Row {
    /// Named in every assertion.
    case: &'static str,
    requested: Option<i32>,
    expected: usize,
}

/// Every class of bound, in-range and out: the published bounds, one past each of them,
/// and both extremes of the modeled `integer` domain.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no bound named",
            requested: None,
            expected: MAX_PAGE_SIZE,
        },
        Row {
            case: "the smallest published bound",
            requested: Some(1),
            expected: 1,
        },
        Row {
            case: "a bound inside the range",
            requested: Some(7),
            expected: 7,
        },
        Row {
            case: "a bound one below the largest published bound",
            requested: Some(99),
            expected: 99,
        },
        Row {
            case: "the largest published bound",
            requested: Some(100),
            expected: MAX_PAGE_SIZE,
        },
        Row {
            case: "one past the largest published bound",
            requested: Some(101),
            expected: MAX_PAGE_SIZE,
        },
        Row {
            case: "the largest value the member's type holds",
            requested: Some(i32::MAX),
            expected: MAX_PAGE_SIZE,
        },
        Row {
            case: "one below the smallest published bound",
            requested: Some(0),
            expected: 1,
        },
        Row {
            case: "one below zero",
            requested: Some(-1),
            expected: 1,
        },
        Row {
            case: "a bound as far below the range as the ceiling is above the floor",
            requested: Some(-100),
            expected: 1,
        },
        Row {
            case: "the smallest value the member's type holds",
            requested: Some(i32::MIN),
            expected: 1,
        },
    ]
}

/// `count` `(sort key, item)` pairs in ascending key order, the shape the cutter takes.
fn listing(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| {
            let key = format!("key-{index:04}");
            (key.clone(), key)
        })
        .collect()
}

/// RULE: a named bound is carried into the page-size range (floored at its minimum,
/// capped at its maximum); only an absent bound takes the default page. Asserted on the
/// shared cutter, which both listings inherit, including the cursor it hands back.
#[test]
fn a_named_page_bound_is_carried_into_the_range_and_an_absent_one_takes_the_default() {
    let scope = state::cursor_scope("accounts", ACCESS_TOKEN);
    let items = listing(TOTAL);

    for row in rows() {
        let Row {
            case,
            requested,
            expected,
        } = row;
        let page = state::page_of(items.clone(), &scope, None, requested)
            .expect("no cursor was presented, so the page cannot be refused");
        let served: Vec<String> = items
            .iter()
            .take(expected)
            .map(|(key, _)| key.clone())
            .collect();
        assert_eq!(
            page.items.len(),
            expected,
            "case {case:?} (maxResults {requested:?}): expected a page of {expected} of \
             {TOTAL} items, got {}",
            page.items.len()
        );
        assert_eq!(
            page.items, served,
            "case {case:?} (maxResults {requested:?}): expected the first {expected} items \
             {served:?}, got {:?}",
            page.items
        );

        // Items were left over in every row (the listing is longer than the largest page
        // the service serves), so each page carries a cursor, and it resumes immediately
        // after the page it came from — under the same bound.
        let cursor = page.next_token.clone().unwrap_or_else(|| {
            panic!(
                "case {case:?} (maxResults {requested:?}): {TOTAL} items were cut to \
                 {expected}, so the page must carry a cursor, got {page:?}"
            )
        });
        let next = state::page_of(items.clone(), &scope, Some(&cursor), requested)
            .expect("the cursor under test is the one this cutter just minted");
        let following: Vec<String> = items
            .iter()
            .skip(expected)
            .take(expected)
            .map(|(key, _)| key.clone())
            .collect();
        assert_eq!(
            next.items, following,
            "case {case:?} (maxResults {requested:?}): resuming must serve the items that \
             follow, {following:?}, got {:?}",
            next.items
        );
    }

    // The defect this rule rules out, stated directly: a caller that NAMED a bound is not
    // served the page of a caller that named none.
    for named in [Some(i32::MIN), Some(-1), Some(0), Some(1)] {
        let bounded = state::page_of(items.clone(), &scope, None, named)
            .expect("no cursor was presented")
            .items
            .len();
        let defaulted = state::page_of(items.clone(), &scope, None, None)
            .expect("no cursor was presented")
            .items
            .len();
        assert!(
            bounded < defaulted,
            "case {named:?}: a named bound below the range must be served the range's \
             floor, not the default page of {defaulted}, got {bounded}"
        );
    }
}

/// An account id for `index`, zero-padded so the ascending listing order is this index
/// order.
fn account_id(index: usize) -> String {
    format!("6{index:011}")
}

/// A role name for `index`, zero-padded for the same reason.
fn role_name(index: usize) -> String {
    format!("Role-{index:03}")
}

/// An emulator holding [`TOTAL`] accounts assigned to one session, with [`TOTAL`] roles
/// assigned to that session's user in the first of them, so both paginated listings are
/// longer than any page.
fn wide_portal() -> Portal {
    let portal = Portal::new();
    let ids: Vec<String> = (0..TOTAL).map(account_id).collect();
    {
        let store = portal.state();
        let mut store = store.lock();
        for (index, id) in ids.iter().enumerate() {
            store.put_account(
                id.clone(),
                format!("Account-{index}"),
                format!("account-{index}@example.com"),
            );
        }
    }
    let assignments: Vec<(String, Vec<String>)> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let roles: Vec<String> = if index == 0 {
                (0..TOTAL).map(role_name).collect()
            } else {
                Vec::new()
            };
            (id.clone(), roles)
        })
        .collect();
    portal.sign_in_assigned(ACCESS_TOKEN, assignments);
    portal
}

/// The `id_key` of every item reported under `items_key`, in the order reported.
fn reported(
    response: &http::Response<bytes::Bytes>,
    items_key: &str,
    id_key: &str,
) -> Vec<String> {
    body_json(response)
        .get(items_key)
        .and_then(|items| items.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|item| item[id_key].as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE over the wire, on both paginated operations: a bound inside the published range
/// bounds the page at exactly that many items, a request naming none is served the
/// default page, and the listing continues from the cursor until every item has been
/// reported once.
#[tokio::test]
async fn both_listings_bound_a_page_by_the_named_bound_and_default_to_the_largest() {
    let portal = wide_portal();
    let accounts: Vec<String> = (0..TOTAL).map(account_id).collect();
    let roles: Vec<String> = (0..TOTAL).map(role_name).collect();

    // `(operation, route, items member, id member, the ids it must report)`.
    let listings: Vec<(&str, String, &str, &str, Vec<String>)> = vec![
        (
            "ListAccounts",
            "/assignment/accounts".to_string(),
            "accountList",
            "accountId",
            accounts.clone(),
        ),
        (
            "ListAccountRoles",
            format!("/assignment/roles?account_id={}", account_id(0)),
            "roleList",
            "roleName",
            roles,
        ),
    ];

    // Bounds a client may actually send: inside the published range, plus the absent
    // bound. Each is paired with the page size the rule says it is served at.
    let bounds: &[(Option<usize>, usize)] = &[
        (None, MAX_PAGE_SIZE),
        (Some(1), 1),
        (Some(37), 37),
        (Some(MAX_PAGE_SIZE), MAX_PAGE_SIZE),
    ];

    for (operation, path, items_key, id_key, expected) in listings {
        let separator = if path.contains('?') { '&' } else { '?' };
        for (bound, page_size) in bounds {
            let case = format!("{operation} with max_result={bound:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut cursor: Option<String> = None;
            // A bound on the walk, so a cursor that fails to advance fails the test.
            for page in 0..TOTAL + 2 {
                let mut uri = path.clone();
                if let Some(bound) = bound {
                    uri.push_str(&format!("{separator}max_result={bound}"));
                }
                if let Some(cursor) = &cursor {
                    let separator = if uri.contains('?') { '&' } else { '?' };
                    uri.push_str(&format!("{separator}next_token={cursor}"));
                }
                let response = portal.get(&uri, Some(ACCESS_TOKEN)).await;
                assert_eq!(
                    response.status(),
                    StatusCode::OK,
                    "case {case:?}: page {page} of {uri:?} must succeed, got {} {:?}",
                    response.status(),
                    body_text(&response)
                );
                let items = reported(&response, items_key, id_key);
                let remaining = expected.len() - seen.len();
                assert_eq!(
                    items.len(),
                    remaining.min(*page_size),
                    "case {case:?}: page {page} must hold {} items, got {}: {items:?}",
                    remaining.min(*page_size),
                    items.len()
                );
                seen.extend(items);
                cursor = body_json(&response)["nextToken"]
                    .as_str()
                    .map(str::to_string);
                if cursor.is_none() {
                    break;
                }
            }
            assert!(
                cursor.is_none(),
                "case {case:?}: the listing did not terminate, {} items seen",
                seen.len()
            );
            assert_eq!(
                seen, expected,
                "case {case:?}: paging must report every item exactly once, got {} items",
                seen.len()
            );
        }
    }
}
