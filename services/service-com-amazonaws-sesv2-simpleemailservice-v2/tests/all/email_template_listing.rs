// SPDX-License-Identifier: CC0-1.0

//! The email-template listing, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this service
//! registered — through the shared harness in the sibling [`crate::configuration_sets`]
//! suite, so routing, deserialization, the handler and serialization are all exercised.
//!
//! The rule these cases pin: `ListEmailTemplates` REPORTS the name and creation instant of
//! every email template the ACCOUNT holds, read off the records the create path persisted
//! (so the listing, each template's own read and the other SES API cannot disagree),
//! ordered by `TemplateName` and windowed by the request's `PageSize` with a `NextToken`
//! that resumes the walk where the page was cut; a `PageSize` outside the range the member
//! documents, and a token no listing of the account's templates handed out, are refused
//! `BadRequestException`, and no call changes what a later caller sees.
//!
//! The tables below span what the rule generalises over: the number of templates stored
//! (none, one, several, more than the default page size), the order they were created in
//! against the order they are reported in, the form of their names (one character,
//! punctuation, mixed case, non-ASCII, a long name), every page size from one entry to one
//! past the whole store plus the absent one, each published page-size bound and one past
//! it, deleting a template out of the listing, and every refusal leg.
//!
//! What a token issued by the CLASSIC SES surface's `ListTemplates` does to this listing —
//! it continues it, because the two APIs page one list of the account's templates — needs
//! both services on one engine and is asserted in
//! `core/router/tests/all/ses_templates_shared_across_v1_and_v2.rs`.

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};
use crate::contact_lists::query_escaped;

/// The page size the listing serves when the request names none: "Pass this token to a
/// subsequent ListEmailTemplates call to retrieve the next 10 email templates"
/// (`smithy_query(query="operation",operation="ListEmailTemplates")`, the output's
/// `NextToken`).
const DEFAULT_PAGE_SIZE: usize = 10;

/// The page sizes `ListEmailTemplatesRequest$PageSize` admits: "The value you specify has
/// to be at least 1, and can be no more than 100."
const PAGE_SIZE_BOUNDS: (i32, i32) = (1, 100);

/// Convenience calls for the email-template surface, over the shared harness.
trait Templates {
    async fn create_template(&self, name: &str, subject: &str) -> Reply;
    async fn delete_template(&self, name: &str) -> Reply;
    async fn list_templates(&self, page_size: Option<i32>, next_token: Option<&str>) -> Reply;
}

impl Templates for Ses {
    async fn create_template(&self, name: &str, subject: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/templates",
            Some(json!({
                "TemplateName": name,
                "TemplateContent": {"Subject": subject, "Text": format!("Text of {name}")},
            })),
        )
        .await
    }

    async fn delete_template(&self, name: &str) -> Reply {
        self.call("DELETE", &format!("/v2/email/templates/{name}"), None)
            .await
    }

    /// `ListEmailTemplates` as an SDK sends it: a GET over the template collection with
    /// the paging members in the query string, escaped the way an SDK escapes them.
    async fn list_templates(&self, page_size: Option<i32>, next_token: Option<&str>) -> Reply {
        let mut query: Vec<String> = Vec::new();
        if let Some(page_size) = page_size {
            query.push(format!("PageSize={page_size}"));
        }
        if let Some(next_token) = next_token {
            query.push(format!("NextToken={}", query_escaped(next_token)));
        }
        let path = if query.is_empty() {
            "/v2/email/templates".to_string()
        } else {
            format!("/v2/email/templates?{}", query.join("&"))
        };
        self.call("GET", &path, None).await
    }
}

/// The (name, CreatedTimestamp) pairs a reply reported, in the order reported. Asserts the
/// call succeeded and that every entry carries both members.
fn reported(case: &str, reply: &Reply) -> Vec<(String, f64)> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} {:?}",
        reply.status, reply.message()
    );
    reply
        .member("TemplatesMetadata")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            let name = entry
                .get("TemplateName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an entry must carry TemplateName, got {entry:?}")
                })
                .to_string();
            let created = entry
                .get("CreatedTimestamp")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an entry must carry CreatedTimestamp, got {entry:?}")
                });
            (name, created)
        })
        .collect()
}

/// The `NextToken` a reply carried, or `None` when it carried none.
fn next_token(reply: &Reply) -> Option<String> {
    reply
        .member("NextToken")
        .as_str()
        .map(str::to_string)
        .filter(|token| !token.is_empty())
}

/// Every page of the listing at `page_size`, concatenated, following each `NextToken`
/// until none comes back. Also asserts that no page is larger than the page size asked
/// for and that a page which is not the last is full.
async fn every_page(ses: &Ses, case: &str, page_size: Option<i32>) -> Vec<(String, f64)> {
    let expected_size = page_size.map_or(DEFAULT_PAGE_SIZE, |size| size as usize);
    let mut seen: Vec<(String, f64)> = Vec::new();
    let mut token: Option<String> = None;
    // Bounded above every page count these cases can ask for, so a listing that never
    // terminates fails here instead of looping forever.
    for round in 0..1_000 {
        let reply = ses.list_templates(page_size, token.as_deref()).await;
        let page = reported(&format!("{case} page {round}"), &reply);
        assert!(
            page.len() <= expected_size,
            "case {case:?} page {round}: expected at most {expected_size} entries, got {}",
            page.len()
        );
        let next = next_token(&reply);
        assert!(
            next.is_none() || page.len() == expected_size,
            "case {case:?} page {round}: a page that is not the last must be full, got {} of \
             {expected_size}",
            page.len()
        );
        seen.extend(page);
        match next {
            None => return seen,
            Some(next) => token = Some(next),
        }
    }
    panic!("case {case:?}: the listing never ran out of pages");
}

/// `template-<n>` for each of `count` templates, in an order whose lexicographic sort
/// differs from the insertion order.
fn template_names(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("template-{:04}", count - index))
        .collect()
}

/// Create every name in `names`, in the order given, and fail loudly if any create is
/// refused: a listing case is only about the listing.
async fn stored(ses: &Ses, case: &str, names: &[String]) {
    for name in names {
        let created = ses
            .create_template(name, &format!("Subject of {name}"))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 creating template {name:?}, got {} {:?}",
            created.status,
            created.message()
        );
    }
}

#[tokio::test]
async fn an_account_with_no_templates_lists_nothing() {
    let ses = Ses::new();
    for page_size in [None, Some(1), Some(PAGE_SIZE_BOUNDS.1)] {
        let case = format!("PageSize={page_size:?} with nothing stored");
        let reply = ses.list_templates(page_size, None).await;
        let page = reported(&case, &reply);
        assert!(
            page.is_empty(),
            "case {case:?}: expected no templates, got {page:?}"
        );
        assert_eq!(
            next_token(&reply),
            None,
            "case {case:?}: expected no NextToken, got {:?}",
            next_token(&reply)
        );
    }
}

#[tokio::test]
async fn paging_visits_every_stored_template_exactly_once_at_every_page_size() {
    // More templates than the default page size, so the default is exercised too.
    const STORED: usize = 23;
    let ses = Ses::new();
    let names = template_names(STORED);
    let expected_order: Vec<String> = {
        let mut sorted = names.clone();
        sorted.sort();
        sorted
    };
    stored(&ses, "paging", &names).await;

    // The whole store on one page is the reference the other page sizes are compared
    // against, so the creation instant of each entry is read off the store once.
    let whole = every_page(&ses, "the whole store", Some(STORED as i32)).await;
    assert_eq!(
        whole.iter().map(|(name, _)| name.clone()).collect::<Vec<String>>(),
        expected_order,
        "case the whole store: expected every template once, in name order, got {whole:?}"
    );

    // EVERY page size from one entry to one past the whole store, plus the absent one, so
    // the rule is asserted over the whole class rather than at one size.
    let sizes: Vec<Option<i32>> = std::iter::once(None)
        .chain((1..=(STORED + 1)).map(|size| Some(size as i32)))
        .collect();
    for page_size in sizes {
        let case = format!("PageSize={page_size:?}");
        let seen = every_page(&ses, &case, page_size).await;
        assert_eq!(
            seen, whole,
            "case {case:?}: expected every stored template once, in name order, with the \
             creation instant the store holds, got {seen:?}"
        );
    }

    // The default page size is the documented 10, not the whole store.
    let reply = ses.list_templates(None, None).await;
    let first_page = reported("absent PageSize", &reply);
    assert_eq!(
        first_page.len(),
        DEFAULT_PAGE_SIZE,
        "case absent PageSize: expected the documented default page size, got {}",
        first_page.len()
    );
    assert!(
        next_token(&reply).is_some(),
        "case absent PageSize: {STORED} stored templates do not fit one page, so a NextToken \
         must come back"
    );
}

#[tokio::test]
async fn templates_of_every_name_form_are_listed_in_name_order() {
    // The rows span the forms an `EmailTemplateName` takes: one character, punctuation,
    // mixed case, a long name and a non-ASCII name.
    let names: Vec<String> = [
        "T",
        "Order.Receipt_v2-2026__the-one-that-is-sixty-four-characters-long",
        "receipts",
        "Receipts",
        "receipts-v2",
        "receipts.v2",
        "領収書",
    ]
    .iter()
    .map(|name| (*name).to_string())
    .collect();
    let ses = Ses::new();
    stored(&ses, "name forms", &names).await;

    let mut expected = names.clone();
    expected.sort();
    for page_size in [None, Some(1), Some(2), Some(names.len() as i32)] {
        let case = format!("name forms at PageSize={page_size:?}");
        let seen = every_page(&ses, &case, page_size).await;
        assert_eq!(
            seen.iter().map(|(name, _)| name.clone()).collect::<Vec<String>>(),
            expected,
            "case {case:?}: expected every name once, in order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn the_listing_reports_each_templates_own_creation_instant() {
    let ses = Ses::new();
    let names = template_names(4);
    stored(&ses, "creation instants", &names).await;

    let listed = every_page(&ses, "creation instants", None).await;
    // A listing is a REPORT: it reports each template the account holds with the instant
    // the record carries, so every entry is a template whose own read answers, and a
    // second listing reports exactly the same entries.
    for (name, created) in &listed {
        let read = ses
            .call("GET", &format!("/v2/email/templates/{name}"), None)
            .await;
        assert_eq!(
            read.status, 200,
            "case {name:?}: expected the listed template to be readable, got {} {:?}",
            read.status, read.message()
        );
        assert!(
            *created > 0.0,
            "case {name:?}: expected the creation instant the record carries, got {created}"
        );
    }
    let again = every_page(&ses, "creation instants again", None).await;
    assert_eq!(
        again, listed,
        "case a second listing: expected the same entries with the same instants, got \
         {again:?}"
    );
}

#[tokio::test]
async fn a_deleted_template_leaves_the_listing_and_the_rest_still_pages() {
    let ses = Ses::new();
    let names = template_names(7);
    stored(&ses, "deletion", &names).await;
    let mut expected: Vec<String> = names.clone();
    expected.sort();

    // Deleting each template in turn: every page size still visits exactly the templates
    // that remain, so a cursor is a position in the order rather than an index into a
    // fixed list.
    while let Some(gone) = expected.first().cloned() {
        let deleted = ses.delete_template(&gone).await;
        assert_eq!(
            deleted.status, 200,
            "case delete {gone:?}: expected HTTP 200, got {} {:?}",
            deleted.status,
            deleted.message()
        );
        expected.remove(0);
        for page_size in [None, Some(1), Some(3)] {
            let case = format!("after deleting {gone:?} at PageSize={page_size:?}");
            let seen = every_page(&ses, &case, page_size).await;
            assert_eq!(
                seen.iter().map(|(name, _)| name.clone()).collect::<Vec<String>>(),
                expected,
                "case {case:?}: expected only the templates that remain, got {seen:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_page_size_outside_the_documented_range_is_refused() {
    let ses = Ses::new();
    let names = template_names(5);
    stored(&ses, "page size bounds", &names).await;
    let (floor, ceiling) = PAGE_SIZE_BOUNDS;

    // Each published bound is served, and the one page size past each is refused.
    for page_size in [floor, floor + 1, ceiling - 1, ceiling] {
        let case = format!("PageSize={page_size} inside the range");
        let reply = ses.list_templates(Some(page_size), None).await;
        let page = reported(&case, &reply);
        assert_eq!(
            page.len(),
            (page_size as usize).min(names.len()),
            "case {case:?}: expected a page of {} entries, got {}",
            (page_size as usize).min(names.len()),
            page.len()
        );
    }
    for page_size in [floor - 1, -1, i32::MIN, ceiling + 1, 1_000, i32::MAX] {
        let case = format!("PageSize={page_size} outside the range");
        let refused = ses.list_templates(Some(page_size), None).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            refused.status, refused.message()
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.message(),
            format!("PageSize {page_size} is outside the supported range of {floor} to {ceiling}."),
            "case {case:?}: expected the refusal to state the value and the range, got {:?}",
            refused.message()
        );
    }
}

#[tokio::test]
async fn a_page_token_no_listing_of_the_accounts_templates_issued_is_refused() {
    let ses = Ses::new();
    let names = template_names(12);
    stored(&ses, "token refusals", &names).await;
    let mut expected: Vec<String> = names.clone();
    expected.sort();

    // The cursor the listing itself hands out is accepted, and resumes where the page it
    // was cut from ended.
    let first = ses.list_templates(Some(5), None).await;
    let first_page = reported("the issued cursor", &first);
    let issued = next_token(&first).expect("a store larger than the page size yields a NextToken");
    let second = ses.list_templates(Some(5), Some(&issued)).await;
    let second_page = reported("the issued cursor", &second);
    assert_eq!(
        second_page
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<String>>(),
        expected[first_page.len()..first_page.len() + 5].to_vec(),
        "case the issued cursor: expected the page after the first, got {second_page:?}"
    );

    // Each row is a value shaped unlike anything a listing of this list issues, or one it
    // issued with its bytes tampered with.
    let tampered = {
        let mut bytes = ::aws_smithy_types::base64::decode(&issued).expect("a token is base64");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        ::aws_smithy_types::base64::encode(&bytes)
    };
    let rows: Vec<(&str, String)> = vec![
        (
            "a token the caller made up",
            "not-a-real-pagination-token".to_string(),
        ),
        ("a template name sent bare", expected[0].clone()),
        ("base64 that carries no authenticator", "AAAAAA==".to_string()),
        ("a tag that does not match the key", tampered),
        (
            "a token truncated by a character",
            issued[..issued.len() - 2].to_string(),
        ),
        ("a token with characters appended", format!("{issued}ff")),
        (
            "a made-up authenticator over a real position",
            ::aws_smithy_types::base64::encode(
                [
                    vec![0_u8; ::service_common::page_tokens::AUTHENTICATOR_BYTES],
                    expected[3].as_bytes().to_vec(),
                ]
                .concat()
                .as_slice(),
            ),
        ),
    ];
    for (case, token) in &rows {
        let refused = ses.list_templates(None, Some(token)).await;
        assert_eq!(
            refused.status, 400,
            "case {case} ({token:?}): expected HTTP 400, got {} {:?}",
            refused.status, refused.message()
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case} ({token:?}): expected BadRequestException, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.message(),
            "Invalid next token.",
            "case {case} ({token:?}): expected this service's one wording for a refused \
             cursor, got {:?}",
            refused.message()
        );
    }

    // A refused token changes nothing: the listing still reports every stored template,
    // and the cursor issued before the refusals still resumes the same page.
    let seen = every_page(&ses, "after the refusals", None).await;
    assert_eq!(
        seen.iter().map(|(name, _)| name.clone()).collect::<Vec<String>>(),
        expected,
        "case after {} refusals: the store must be untouched, got {seen:?}",
        rows.len()
    );
    let resumed = ses.list_templates(Some(5), Some(&issued)).await;
    assert_eq!(
        reported("the issued cursor again", &resumed),
        second_page,
        "case after {} refusals: the issued cursor must still resume the same page",
        rows.len()
    );
}
