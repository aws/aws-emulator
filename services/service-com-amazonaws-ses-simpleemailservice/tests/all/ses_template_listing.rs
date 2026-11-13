// SPDX-License-Identifier: CC0-1.0

//! `ListTemplates`, end to end over the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so each call
//! is a real awsQuery request against the service: form-urlencoded body in, XML
//! out, one shared engine behind it. The templates the listing reports are the
//! ones in this service's store, seeded through the same `state` API the handlers
//! use (`CreateTemplate` over the wire is `tests/all/ses_template_creation.rs`'s
//! subject), so what is
//! asserted is the RULE the operation follows over a SET of inputs rather than one
//! recorded request:
//!
//! * the listing reports the stored templates, with each one's stored creation
//!   instant, in one stable order, and paging with `MaxItems`/`NextToken` visits
//!   every template exactly once for EVERY page size;
//! * a `NextToken` this listing did not issue is refused with HTTP 400
//!   `InvalidParameterValue`, whatever shape the token has, and the store is left
//!   alone;
//! * `MaxItems` follows the bounds the member publishes: a value past the ceiling
//!   is clamped to it, a value below the floor is refused, and an absent value
//!   takes the operation's own default page size.

use crate::harness::{call, element, elements, emulator_over, field, Field};
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_ses_simpleemailservice::state::{
    encode_page_token, recorded_template, service_state, template_page_token,
    template_page_token_key, LISTING_TEMPLATES,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

/// The page size `ListTemplatesRequest$MaxItems` documents as the default: "If you
/// do not specify a value, 10 is the default page size."
const DEFAULT_PAGE_SIZE: usize = 10;

/// The largest page the operation serves: "This value must be at least 1 and less
/// than or equal to 100. If more than 100 items are requested, the page size will
/// automatically set to 100" (`ListTemplatesRequest$MaxItems`).
const MAX_PAGE_SIZE: usize = 100;

/// The epoch second the seeded creation instants start at, so each stored
/// timestamp is a distinct whole second and the value the listing reports can be
/// compared with the one the store holds.
const FIRST_CREATED_AT_SECS: u64 = 1_767_225_600;

/// A router over an engine this suite keeps, holding one template per name in
/// `names` — created a second apart, in the order given, which is NOT the
/// lexicographic order the listing reports.
///
/// Returns the router, the engine behind it (whose account registry mints this
/// listing's cursors) and, per template name, the epoch second it was created at.
fn emulator_holding_over_engine(
    names: &[String],
) -> (
    MultiServiceRouter,
    Arc<engine::InMemoryEngine>,
    Vec<(String, i64)>,
) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let store = service_state(&*engine).expect("this service has a store on the shared engine");
    let mut created = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let secs = FIRST_CREATED_AT_SECS + index as u64;
        let created_at = UNIX_EPOCH + Duration::from_secs(secs);
        let recorded = store
            .lock()
            .put_template(recorded_template(
                name,
                Some(format!("Subject of {name}")),
                Some(format!("Text of {name}")),
                None,
                created_at,
            ))
            .is_some();
        assert!(
            recorded,
            "case {name:?}: a template inside the account's limit must be recorded"
        );
        created.push((name.clone(), secs as i64));
    }
    created.sort();
    (emulator_over(Arc::clone(&engine)), engine, created)
}

/// The same router and creation instants, for a case that needs no engine of its
/// own.
fn emulator_holding(names: &[String]) -> (MultiServiceRouter, Vec<(String, i64)>) {
    let (router, _, created) = emulator_holding_over_engine(names);
    (router, created)
}

/// `name-<index>` for each of `count` templates, in an order whose lexicographic
/// sort differs from the insertion order.
fn template_names(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("template-{:04}", count - index))
        .collect()
}

/// An element's text with the entities the XML codec escapes read back, so an
/// asserted message reads as the client's parser hands it over: a refusal quotes
/// the rejected value in angle brackets, which travel escaped on the wire.
fn unescaped(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        // Last, so an escaped ampersand in the original is not read as the start
        // of another entity.
        .replace("&amp;", "&")
}

/// One `ListTemplates` call: `(status, body)`.
async fn list_templates_raw(
    router: &MultiServiceRouter,
    max_items: Option<i32>,
    next_token: Option<&str>,
) -> (u16, String) {
    let mut params: Vec<Field> = Vec::new();
    if let Some(max_items) = max_items {
        params.push(field("MaxItems", max_items.to_string()));
    }
    if let Some(next_token) = next_token {
        params.push(field("NextToken", next_token));
    }
    call(router, "ListTemplates", &params).await
}

/// `ListTemplates`, as `(the (name, created-at second) pairs on this page, the
/// next token)`. Asserts the call succeeded and that every reported entry carries
/// both members.
async fn list_templates(
    router: &MultiServiceRouter,
    max_items: Option<i32>,
    next_token: Option<&str>,
) -> (Vec<(String, i64)>, Option<String>) {
    let (status, body) = list_templates_raw(router, max_items, next_token).await;
    assert_eq!(
        status, 200,
        "case ListTemplates(max={max_items:?}, token={next_token:?}): expected 200, \
         got {status} with {body}"
    );
    let page = element(&body, "TemplatesMetadata")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default();
    let entries = page
        .iter()
        .map(|member| {
            let name = element(member, "Name").unwrap_or_else(|| {
                panic!("case ListTemplates(max={max_items:?}): an entry carries Name, got {member}")
            });
            let rendered = element(member, "CreatedTimestamp").unwrap_or_else(|| {
                panic!(
                    "case ListTemplates(max={max_items:?}): an entry carries CreatedTimestamp, \
                     got {member}"
                )
            });
            let parsed = DateTime::from_str(&rendered, Format::DateTime).unwrap_or_else(|error| {
                panic!(
                    "case {name:?}: CreatedTimestamp {rendered:?} must be a date-time, \
                     got {error}"
                )
            });
            (name, parsed.secs())
        })
        .collect();
    (entries, element(&body, "NextToken"))
}

/// Every page of the listing at `max_items`, concatenated, following each
/// `NextToken` until none comes back. Also asserts no page is larger than the page
/// size the request asks for.
async fn every_page(
    router: &MultiServiceRouter,
    max_items: Option<i32>,
    page_size: usize,
) -> Vec<(String, i64)> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    // Bounded above every page count this suite can ask for, so a listing that
    // never terminates fails here instead of looping forever.
    for round in 0..1_000 {
        let (page, next) = list_templates(router, max_items, token.as_deref()).await;
        assert!(
            page.len() <= page_size,
            "case max={max_items:?} page {round}: expected at most {page_size} entries, \
             got {}",
            page.len()
        );
        assert!(
            next.is_none() || page.len() == page_size,
            "case max={max_items:?} page {round}: a page that is not the last must be full, \
             got {} of {page_size}",
            page.len()
        );
        seen.extend(page);
        match next {
            None => return seen,
            Some(next) => token = Some(next),
        }
    }
    panic!("case max={max_items:?}: the listing never ran out of pages");
}

#[tokio::test]
async fn an_account_with_no_templates_lists_nothing() {
    let (router, _) = emulator_holding(&[]);
    for max_items in [None, Some(1), Some(100)] {
        let (page, token) = list_templates(&router, max_items, None).await;
        assert!(
            page.is_empty(),
            "case max={max_items:?}: expected no templates, got {page:?}"
        );
        assert_eq!(
            token, None,
            "case max={max_items:?}: expected no NextToken, got {token:?}"
        );
    }
}

#[tokio::test]
async fn paging_visits_every_stored_template_exactly_once_at_every_page_size() {
    // A count larger than the default page size, so the default is exercised too.
    const STORED: usize = 23;
    let (router, expected) = emulator_holding(&template_names(STORED));

    // EVERY page size from one entry to one past the whole store, so the rule is
    // asserted over the whole class rather than at one size.
    let sizes: Vec<Option<i32>> = std::iter::once(None)
        .chain((1..=(STORED + 1)).map(|size| Some(size as i32)))
        .collect();
    for max_items in sizes {
        let page_size = max_items.map_or(DEFAULT_PAGE_SIZE, |size| size as usize);
        let seen = every_page(&router, max_items, page_size).await;
        assert_eq!(
            seen, expected,
            "case max={max_items:?}: expected every stored template once, in name order, \
             with its stored creation instant, got {seen:?}"
        );
    }

    // The default page size is the member's documented 10, not the whole store.
    let (first_page, token) = list_templates(&router, None, None).await;
    assert_eq!(
        first_page.len(),
        DEFAULT_PAGE_SIZE,
        "case absent MaxItems: expected the documented default page size, got {}",
        first_page.len()
    );
    assert!(
        token.is_some(),
        "case absent MaxItems: {STORED} stored templates do not fit one page, so a \
         NextToken must come back"
    );
}

#[tokio::test]
async fn a_page_size_past_the_ceiling_is_clamped_and_one_below_the_floor_is_refused() {
    // More templates than the largest page the operation serves, so a clamp is
    // observable as a short page plus a token.
    const STORED: usize = MAX_PAGE_SIZE + 17;
    let (router, expected) = emulator_holding(&template_names(STORED));

    // At and inside the ceiling the page size is honoured exactly; past it, the
    // member documents the page size being set to the ceiling.
    let rows: &[(i32, usize)] = &[
        (1, 1),
        (2, 2),
        (99, 99),
        (MAX_PAGE_SIZE as i32, MAX_PAGE_SIZE),
        (MAX_PAGE_SIZE as i32 + 1, MAX_PAGE_SIZE),
        (1_000, MAX_PAGE_SIZE),
        (i32::MAX, MAX_PAGE_SIZE),
    ];
    for (max_items, page_size) in rows {
        let (page, token) = list_templates(&router, Some(*max_items), None).await;
        assert_eq!(
            page.len(),
            *page_size,
            "case MaxItems={max_items}: expected a page of {page_size}, got {}",
            page.len()
        );
        assert!(
            token.is_some(),
            "case MaxItems={max_items}: {STORED} stored templates do not fit a page of \
             {page_size}, so a NextToken must come back"
        );
        let seen = every_page(&router, Some(*max_items), *page_size).await;
        assert_eq!(
            seen, expected,
            "case MaxItems={max_items}: paging must still visit every template once"
        );
    }

    // Below the floor the value is refused, naming the member and the value.
    for max_items in [0, -1, -100, i32::MIN] {
        let (status, body) = list_templates_raw(&router, Some(max_items), None).await;
        assert_eq!(
            status, 400,
            "case MaxItems={max_items}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("InvalidParameterValue"),
            "case MaxItems={max_items}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            element(&body, "Message").map(|text| unescaped(&text)).as_deref(),
            Some(format!("Invalid MaxItems <{max_items}>.").as_str()),
            "case MaxItems={max_items}: the message must name the member and the value, \
             got {body}"
        );
    }
}

#[tokio::test]
async fn a_page_token_this_listing_did_not_issue_is_refused() {
    const STORED: usize = 12;
    let (router, engine, expected) = emulator_holding_over_engine(&template_names(STORED));
    let token_key = template_page_token_key(&*engine)
        .expect("the account's email-template registry is on the shared engine");

    // The cursor the listing itself hands out is accepted, and resumes the page
    // after the one it was cut from.
    let (first_page, issued) = list_templates(&router, Some(5), None).await;
    let issued = issued.expect("a store larger than the page size yields a NextToken");
    let (second_page, _) = list_templates(&router, Some(5), Some(&issued)).await;
    assert_eq!(
        second_page,
        expected[first_page.len()..first_page.len() + 5].to_vec(),
        "case the issued cursor: expected the page after the first, got {second_page:?}"
    );

    // Each row is a token shaped unlike anything this listing issues, or one it
    // issued for a different sequence or a tampered position.
    let real = template_page_token(&token_key, &expected[3].0);
    let tampered = {
        let mut bytes = ::aws_smithy_types::base64::decode(&real).expect("an issued token is base64");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        ::aws_smithy_types::base64::encode(&bytes)
    };
    let rows: Vec<(&str, String)> = vec![
        ("a token the caller made up", "not-a-real-pagination-token".to_string()),
        ("an empty token", String::new()),
        ("a template name sent bare", expected[0].0.clone()),
        ("hex that carries no key", "0011223344556677".to_string()),
        ("hex too short to carry a tag", "00112233".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("a tag that does not match the key", tampered),
        ("a token truncated by a byte", real[..real.len() - 2].to_string()),
        ("a token with a byte appended", format!("{real}ff")),
        (
            "a cursor of another listing",
            encode_page_token("ListIdentities", &expected[3].0),
        ),
        (
            "a cursor of this surface's own other listings",
            encode_page_token(LISTING_TEMPLATES, &expected[3].0),
        ),
        // A rejected value is quoted back into the error envelope, so one
        // carrying XML metacharacters must travel escaped rather than breaking
        // the envelope it is rendered into.
        (
            "a token carrying XML metacharacters",
            "<Message>&\"'".to_string(),
        ),
    ];
    for (case, token) in &rows {
        let (status, body) = list_templates_raw(&router, None, Some(token)).await;
        assert_eq!(
            status, 400,
            "case {case} ({token:?}): expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("InvalidParameterValue"),
            "case {case} ({token:?}): expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            element(&body, "Message").map(|text| unescaped(&text)).as_deref(),
            Some(format!("Invalid PageToken <{token}>.").as_str()),
            "case {case} ({token:?}): the message must name the rejected token, got {body}"
        );
        assert!(
            !body.contains(token.as_str()) || !token.contains(['<', '>', '&']),
            "case {case} ({token:?}): an XML metacharacter must be escaped in the \
             envelope, got {body}"
        );
    }

    // A refused token changes nothing: the listing still reports every stored
    // template, and the cursor issued before the refusals still resumes.
    let seen = every_page(&router, None, DEFAULT_PAGE_SIZE).await;
    assert_eq!(
        seen, expected,
        "case after {} refusals: the store must be untouched, got {seen:?}",
        rows.len()
    );
    let (resumed, _) = list_templates(&router, Some(5), Some(&issued)).await;
    assert_eq!(
        resumed, second_page,
        "case after {} refusals: the issued cursor must still resume the same page, \
         got {resumed:?}",
        rows.len()
    );

    // A cursor IS accepted over a position the store no longer holds: the token
    // names where the next page starts, so the listing resumes at the first
    // template at or after it rather than refusing a caller whose page boundary
    // was deleted between calls.
    let resume_at = &expected[7].0;
    let over_a_gap = template_page_token(&token_key, &format!("{resume_at}-gone"));
    let (after_gap, _) = list_templates(&router, None, Some(&over_a_gap)).await;
    assert_eq!(
        after_gap,
        expected[8..].to_vec(),
        "case a cursor over a position the store does not hold: expected the templates \
         after it, got {after_gap:?}"
    );
}
