// SPDX-License-Identifier: CC0-1.0

//! A page token resumes a portal listing only in the ONE spelling this service issued.
//!
//! The rule under test: a page cursor is opaque, so a position in a listing is named by
//! the single token the service minted for it and by nothing else — a value that merely
//! decodes to the same cursor bytes, while being a string no response of this service
//! ever carried, is a token this service did not issue and is refused with
//! `InvalidRequestException` (HTTP 400) rather than answered with the page that follows.
//! Hex is written out in lower case but read back case-insensitively, so re-spelling an
//! issued token's letter digits in upper case is exactly such a value: the same bytes in
//! a spelling the mint cannot write.
//!
//! The rule belongs to the cursor codec `src/state.rs` shares, so the rows are every
//! paginated operation of this service, and they differ along what the rule generalises
//! over: which listing issued the cursor (the session's accounts, one account's roles),
//! what the cursor's key is (an account id, a role name), how far into the listing the
//! cursor was issued (every cursor the listing hands out, not only the first), the page
//! size it was issued at, and how many of the token's letter digits are re-spelt and
//! where (all of them, the first, the last, every other one).
//!
//! Each row also asserts the positive control: the token AS ISSUED still resumes, and
//! resumes with exactly the entries that follow the page it came from — so the refusals
//! above are earned by the spelling alone.
//!
//! The harness is `portal_session`'s, so every suite here speaks to one mounted router.

use bytes::Bytes;
use http::{Response, StatusCode};
use std::collections::BTreeSet;

use crate::portal_session::{body_json, body_text, error_code, error_message, Portal};

/// The one wording both listings refuse a token they did not issue with
/// (`src/ops/list_accounts.rs`, `src/ops/list_account_roles.rs`).
const REFUSAL: &str = "nextToken is not a page token this operation issued for this listing";

/// The sign-in session this suite's listings are read with.
const ACCESS_TOKEN: &str = "tok-spelling";

/// The accounts this suite assigns the session, and the roles assigned in each:
/// `(account id, name, email, roles)`.
///
/// There are more accounts, and more roles in the first two accounts, than any page size
/// below, so each listing hands out at least one cursor; the role counts and the
/// role-name shapes differ so a role listing is cut at more than one kind of key.
const ASSIGNED: &[(&str, &str, &str, &[&str])] = &[
    (
        "501111111111",
        "Spelling-Alpha",
        "alpha@example.com",
        &[
            "AdministratorAccess",
            "Billing-Audit",
            "ReadOnly",
            "ZZZ-Break-Glass",
        ],
    ),
    (
        "502222222222",
        "Spelling-Beta",
        "beta@example.com",
        &["Delta_Access", "ReadOnly", "Sandbox-Role1"],
    ),
    (
        "503333333333",
        "Spelling-Gamma",
        "gamma@example.com",
        &["AdministratorAccess"],
    ),
    (
        "504444444444",
        "Spelling-Delta",
        "delta@example.com",
        &["Audit_Role-1", "ReadOnly"],
    ),
];

/// The page sizes every listing is read at: one entry per page, and more than one, so the
/// cursors under test carry keys from more than one position in the listing.
const PAGE_SIZES: [usize; 2] = [1, 2];

/// One paginated read of this service, pointed at the set [`ASSIGNED`] seeds for it.
struct Listing {
    /// The operation under test, named in every assertion.
    operation: &'static str,
    /// The output member the page is reported in.
    items: &'static str,
    /// The member of an item that identifies it, and the key its cursor walks.
    id_key: &'static str,
    /// The route and the members that pick the listing out, without a page size or a
    /// cursor.
    path: String,
    /// The ids this listing reports, in the order it reports them.
    expected: Vec<String>,
}

/// An emulator with [`ASSIGNED`] seeded and the session signed in to all of it.
fn portal() -> Portal {
    let portal = Portal::new();
    {
        let store = portal.state();
        let mut store = store.lock();
        for (account_id, name, email, _) in ASSIGNED {
            store.put_account(*account_id, *name, *email);
        }
    }
    let assignments: Vec<(&str, Vec<String>)> = ASSIGNED
        .iter()
        .map(|(account_id, ..)| (*account_id, roles_of(account_id)))
        .collect();
    portal.sign_in_assigned(ACCESS_TOKEN, assignments);
    portal
}

/// The roles [`ASSIGNED`] puts in `account_id`, in the ascending order a role listing
/// reports them in.
fn roles_of(account_id: &str) -> Vec<String> {
    ASSIGNED
        .iter()
        .find(|(id, ..)| *id == account_id)
        .map(|(.., roles)| {
            roles
                .iter()
                .map(|role| (*role).to_string())
                .collect::<BTreeSet<String>>()
                .into_iter()
                .collect()
        })
        .unwrap_or_default()
}

/// Every paginated operation of this service, each with the ids it must report.
fn listings() -> Vec<Listing> {
    let accounts: Vec<String> = {
        let mut ids: Vec<String> = ASSIGNED.iter().map(|(id, ..)| (*id).to_string()).collect();
        ids.sort();
        ids
    };
    // The account whose role listing is longest, so its cursors are issued at several
    // positions, and a second account so a role cursor's key is not one role name only.
    let mut rows = vec![Listing {
        operation: "ListAccounts",
        items: "accountList",
        id_key: "accountId",
        path: "/assignment/accounts".to_string(),
        expected: accounts,
    }];
    for (account_id, ..) in &ASSIGNED[..2] {
        rows.push(Listing {
            operation: "ListAccountRoles",
            items: "roleList",
            id_key: "roleName",
            path: format!("/assignment/roles?account_id={account_id}"),
            expected: roles_of(account_id),
        });
    }
    rows
}

/// The request reading `listing` at `page_size`, resuming from `token` when there is one.
fn uri(listing: &Listing, page_size: usize, token: Option<&str>) -> String {
    let separator = if listing.path.contains('?') { '&' } else { '?' };
    let mut uri = format!("{}{separator}max_result={page_size}", listing.path);
    if let Some(token) = token {
        uri.push_str(&format!("&next_token={token}"));
    }
    uri
}

/// The `id_key` of every item `listing` reported in `response`, in order.
fn reported(listing: &Listing, response: &Response<Bytes>) -> Vec<String> {
    body_json(response)
        .get(listing.items)
        .and_then(|items| items.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|item| item[listing.id_key].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every cursor `listing` hands out when walked at `page_size`, in the order issued.
async fn issued_cursors(portal: &Portal, listing: &Listing, page_size: usize) -> Vec<String> {
    let case = format!("{} at max_result={page_size}", listing.operation);
    let mut cursors: Vec<String> = Vec::new();
    let mut next: Option<String> = None;
    // A bound, so a cursor that fails to advance fails the test instead of looping.
    for _ in 0..16_u32 {
        let response = portal
            .get(&uri(listing, page_size, next.as_deref()), Some(ACCESS_TOKEN))
            .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "case {case:?}: expected the walk to answer 200, got {} {:?}",
            response.status(),
            body_text(&response)
        );
        match body_json(&response)["nextToken"].as_str() {
            Some(token) => {
                cursors.push(token.to_string());
                next = Some(token.to_string());
            }
            None => break,
        }
    }
    assert!(
        !cursors.is_empty(),
        "case {case:?}: the seeded listing is longer than one page, so it must issue a cursor"
    );
    cursors
}

/// Re-spell `token` by upper-casing the hex digits whose letter positions `select`
/// accepts, leaving every other character exactly as issued.
fn respell(token: &str, select: impl Fn(usize) -> bool) -> String {
    let mut letters = 0usize;
    token
        .chars()
        .map(|digit| {
            if digit.is_ascii_alphabetic() {
                let position = letters;
                letters += 1;
                if select(position) {
                    return digit.to_ascii_uppercase();
                }
            }
            digit
        })
        .collect()
}

/// Every re-spelling of `token` under test: what the case is, and the token as re-spelt.
fn respellings(case: &str, token: &str) -> Vec<(String, String)> {
    let letters = token.chars().filter(char::is_ascii_alphabetic).count();
    assert!(
        letters >= 2,
        "case {case:?}: an issued token must carry letter digits to re-spell, got {letters} in \
         {token:?}"
    );
    vec![
        (
            "every letter digit re-spelt".to_string(),
            token.to_ascii_uppercase(),
        ),
        (
            "the first letter digit re-spelt".to_string(),
            respell(token, |position| position == 0),
        ),
        (
            "the last letter digit re-spelt".to_string(),
            respell(token, |position| position == letters - 1),
        ),
        (
            "every other letter digit re-spelt".to_string(),
            respell(token, |position| position % 2 == 0),
        ),
    ]
}

/// Every token a portal listing issues is lower-case hex, and a re-spelling of it carries
/// the SAME cursor bytes — so nothing but how its digits are written separates it from the
/// token this service handed out.
#[tokio::test]
async fn a_respelt_page_token_carries_the_issued_cursor_bytes_in_a_spelling_the_mint_never_writes()
{
    let portal = portal();

    for listing in listings() {
        for page_size in PAGE_SIZES {
            for issued in issued_cursors(&portal, &listing, page_size).await {
                let case = format!("{} at max_result={page_size}", listing.operation);
                assert_eq!(
                    issued,
                    issued.to_ascii_lowercase(),
                    "case {case:?}: an issued token is lower-case hex, got {issued:?}"
                );
                let issued_bytes = hex::decode(&issued).unwrap_or_else(|error| {
                    panic!("case {case:?}: an issued token is hex, got {issued:?} ({error})")
                });
                for (why, respelt) in respellings(&case, &issued) {
                    let case = format!("{case} with {why}");
                    assert_ne!(
                        respelt, issued,
                        "case {case:?}: the re-spelling must differ from the issued token"
                    );
                    let respelt_bytes = hex::decode(&respelt).unwrap_or_else(|error| {
                        panic!(
                            "case {case:?}: a re-spelling is still hex, got {respelt:?} ({error})"
                        )
                    });
                    assert_eq!(
                        respelt_bytes, issued_bytes,
                        "case {case:?}: expected the issued cursor bytes, got {respelt_bytes:?}"
                    );
                }
            }
        }
    }
}

/// Both paginated operations refuse a re-spelt page token with 400
/// `InvalidRequestException` and the service's unissued-token wording, carrying no page,
/// no cursor and no echo of the rejected token — while the token as issued resumes with
/// exactly the entries that follow the page it came from.
#[tokio::test]
async fn every_paginated_listing_refuses_a_respelt_page_token_and_resumes_the_issued_one() {
    let portal = portal();

    for listing in listings() {
        for page_size in PAGE_SIZES {
            let cursors = issued_cursors(&portal, &listing, page_size).await;
            for (index, issued) in cursors.iter().enumerate() {
                let case = format!(
                    "{} at max_result={page_size}, cursor {} of {}",
                    listing.operation,
                    index + 1,
                    cursors.len()
                );

                for (why, respelt) in respellings(&case, issued) {
                    let case = format!("{case} with {why}");
                    let response = portal
                        .get(&uri(&listing, page_size, Some(&respelt)), Some(ACCESS_TOKEN))
                        .await;
                    assert_eq!(
                        (response.status(), error_code(&response).as_str()),
                        (StatusCode::BAD_REQUEST, "InvalidRequestException"),
                        "case {case:?}: expected 400 InvalidRequestException for a token this \
                         service never minted, got {} {:?}",
                        response.status(),
                        body_text(&response)
                    );
                    let message = error_message(&response);
                    assert_eq!(
                        message, REFUSAL,
                        "case {case:?}: expected the message {REFUSAL:?}, got {message:?}"
                    );
                    assert!(
                        !message.contains(&respelt),
                        "case {case:?}: the refusal must not echo the rejected token, got \
                         {message:?}"
                    );
                    let body = body_json(&response);
                    assert!(
                        body.get(listing.items).is_none() && body.get("nextToken").is_none(),
                        "case {case:?}: a refused cursor carries no page and no cursor, got {body}"
                    );
                }

                // The positive control: the token as this service spelt it resumes, with
                // the entries that follow the page that issued it.
                let response = portal
                    .get(&uri(&listing, page_size, Some(issued)), Some(ACCESS_TOKEN))
                    .await;
                assert_eq!(
                    response.status(),
                    StatusCode::OK,
                    "case {case:?} resuming from the issued token: expected 200, got {} {:?}",
                    response.status(),
                    body_text(&response)
                );
                let start = (index + 1) * page_size;
                let expected: Vec<String> = listing
                    .expected
                    .iter()
                    .skip(start)
                    .take(page_size)
                    .cloned()
                    .collect();
                let served = reported(&listing, &response);
                assert_eq!(
                    served, expected,
                    "case {case:?} resuming from the issued token: expected {expected:?}, got \
                     {served:?}"
                );
            }
        }
    }
}
