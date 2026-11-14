// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetUsagePlanKeys`, the API keys one usage plan meters.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The RULES under test, each over a SET of inputs rather than one recorded call:
//!
//! * the collection reports the plan's STORED associations in the order the keys were
//!   added, each rendered exactly as that pair's own `GetUsagePlanKey` read reports it,
//!   over a table of API keys spanning the members a key may and may not carry;
//! * it reports one `position`/`limit` page of that sequence at a time, over page sizes
//!   across the documented range (default 25, maximum 500), each published bound and one
//!   past each, crossed with every cursor the sequence holds, and a cursor naming nothing
//!   in the sequence is a bad request rather than an empty page;
//! * `nameQuery` narrows the sequence to the keys whose name begins with it, in the same
//!   order, and pages through the narrowed sequence;
//! * a `usagePlanId` naming no stored plan is refused 404 rather than served an empty
//!   page, and an association whose API key is gone is on no page;
//! * a read changes nothing a later caller sees.
//!
//! [`Gateway`], [`Answer`], [`assert_failure`], [`string_member`] and [`page_items`] are
//! this crate's one wire-test harness, reused rather than standing a second emulator up
//! another way; `create_usage_plan`, `create_key` and
//! `create_usage_plan_key`/`get_usage_plan_key` are the usage plan, API key and usage plan
//! key suites' one spelling of those calls.

use serde_json::{json, Value};

use crate::api_key_lifecycle::create_key;
use crate::rest_api_lifecycle::{
    assert_failure, page_items, string_member, Answer, Gateway,
};
use crate::usage_plan_key_lifecycle::{create_usage_plan_key, get_usage_plan_key};
use crate::usage_plan_lifecycle::create_usage_plan;

/// The default page size API Gateway applies when a request names no `limit`, and the
/// largest it accepts — from `GetUsagePlanKeysRequest$limit`'s own documentation, "The
/// default value is 25 and the maximum value is 500"
/// (`smithy_query(query="doc", shape="GetUsagePlanKeysRequest", member="limit")`).
const DEFAULT_PAGE_LIMIT: usize = 25;
const MAX_PAGE_LIMIT: i64 = 500;

/// The one key type a `UsagePlanKey` takes: from `UsagePlanKey$type`'s own documentation,
/// "Currently, the valid key type is `API_KEY`"
/// (`smithy_query(query="doc", shape="UsagePlanKey", member="type")`).
const API_KEY_TYPE: &str = "API_KEY";

/// AWS's published minimum length for a caller-supplied API key value, as the sibling
/// `usage_plan_key_lifecycle` suite documents it: 20 characters.
const MIN_VALUE_CHARS: usize = 20;

/// A caller-supplied API key value of the published minimum length, distinct per `n`: an
/// API key's value is unique per account, so two rows cannot supply the same one.
fn supplied_value(n: usize) -> String {
    let distinct = format!("{n}");
    format!("{}{distinct}", "v".repeat(MIN_VALUE_CHARS - distinct.len()))
}

/// `GetUsagePlanKeys` on `plan`, with `query` appended when it is not empty.
///
/// `pub(crate)` so a sibling suite reads the collection through this one spelling of the
/// call rather than keeping its own copy (the `usage_plan_state_and_refusals` suite does,
/// to see what a `DeleteUsagePlanKey` removed from it).
pub(crate) async fn list_keys(gateway: &Gateway, plan: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("/usageplans/{plan}/keys")
    } else {
        format!("/usageplans/{plan}/keys?{query}")
    };
    gateway.call("GET", &uri, None).await
}

/// `GetUsagePlanKeys` on `plan` with `query`, asserting the modelled 200 and returning the
/// body.
async fn listed(gateway: &Gateway, plan: &str, query: &str) -> Value {
    let answer = list_keys(gateway, plan, query).await;
    assert_eq!(
        answer.status, 200,
        "case GetUsagePlanKeys({plan}, {query:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// The `id` of every usage plan key on a page, in page order.
fn listed_ids(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| string_member(item, "id", case))
        .collect()
}

/// `CreateUsagePlanKey` for an `API_KEY`, asserting the modelled 201 and returning the
/// body.
async fn meter_key(gateway: &Gateway, plan: &str, key_id: &str) -> Value {
    let case = format!("CreateUsagePlanKey({plan}, {key_id})");
    let answer = create_usage_plan_key(gateway, plan, key_id, API_KEY_TYPE).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// A plan with no association, for the suites that build their own.
async fn bare_plan(gateway: &Gateway, name: &str) -> String {
    let plan = create_usage_plan(gateway, name, json!({ "name": name })).await;
    string_member(&plan, "id", name)
}

/// One row of the key table: the case name, and the `CreateApiKey` request built from the
/// row's index (see [`supplied_value`]).
type KeyRow = (&'static str, fn(usize) -> Value);

/// A table of API keys spanning what the rendered `UsagePlanKey` generalises over: which
/// members the key carries (so `name` is present on some and absent on others), whether
/// its value is caller-supplied or minted, and whether it is enabled.
fn key_table() -> Vec<KeyRow> {
    vec![
        ("an unnamed key", |_| json!({})),
        ("a named key", |n| {
            json!({ "name": format!("collection-key-{n}") })
        }),
        ("a named and described key", |n| {
            json!({ "name": format!("described-key-{n}"), "description": "the described key" })
        }),
        ("a disabled key", |n| {
            json!({ "name": format!("disabled-key-{n}"), "enabled": false })
        }),
        ("a key with a caller-supplied value", |n| {
            json!({ "name": format!("imported-key-{n}"), "value": supplied_value(n) })
        }),
        ("a key with a marketplace customer", |n| {
            json!({ "customerId": format!("marketplace-customer-{n}") })
        }),
    ]
}

/// Create every row of [`key_table`] as an API key and meter it against `plan`, answering
/// the key ids in the order they were added.
async fn metered_table(gateway: &Gateway, plan: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for (index, (case, request)) in key_table().into_iter().enumerate() {
        let key = create_key(gateway, case, request(index)).await;
        let key_id = string_member(&key, "id", case);
        meter_key(gateway, plan, &key_id).await;
        ids.push(key_id);
    }
    ids
}

#[tokio::test]
async fn get_usage_plan_keys_reports_every_metered_key_as_its_own_read_does() {
    let gateway = Gateway::new();
    let plan = bare_plan(&gateway, "key-collection-plan").await;

    // A plan that meters no key has an EMPTY collection, not a missing one.
    let empty = listed(&gateway, &plan, "").await;
    assert!(
        listed_ids(&empty, "GetUsagePlanKeys(no keys)").is_empty(),
        "case GetUsagePlanKeys(no keys): expected no items, got {empty:?}"
    );
    assert!(
        empty["position"].is_null(),
        "case GetUsagePlanKeys(no keys): the last page omits position, got {:?}",
        empty["position"]
    );

    let added = metered_table(&gateway, &plan).await;

    let all = listed(&gateway, &plan, "").await;
    let ids = listed_ids(&all, "GetUsagePlanKeys(all)");
    assert_eq!(
        ids, added,
        "case GetUsagePlanKeys(all): expected the keys in the order they were added \
         {added:?}, got {ids:?}"
    );

    // Each listed association is the SAME record the pair's own read reports — the
    // collection renders the stored association, it does not build a second rendering of
    // it.
    let items = page_items(&all, "GetUsagePlanKeys(all)");
    for (key_id, item) in added.iter().zip(items.iter()) {
        let case = format!("GetUsagePlanKey({plan}, {key_id})");
        let read = get_usage_plan_key(&gateway, &plan, key_id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            item, &read.body,
            "case {case}: the listed association must equal its own read {:?}, got {item:?}",
            read.body
        );
    }

    // A listing with no `limit` reports the default page size at most.
    assert!(
        items.len() <= DEFAULT_PAGE_LIMIT,
        "case GetUsagePlanKeys(all): a default page holds at most {DEFAULT_PAGE_LIMIT} \
         items, got {}",
        items.len()
    );

    // A SECOND plan metering a subset of the same keys reports only its own
    // associations: the collection is the plan's, not the account's.
    let other = bare_plan(&gateway, "key-collection-other").await;
    let subset: Vec<String> = added.iter().skip(2).take(2).cloned().collect();
    for key_id in &subset {
        meter_key(&gateway, &other, key_id).await;
    }
    let other_ids = listed_ids(
        &listed(&gateway, &other, "").await,
        "GetUsagePlanKeys(other plan)",
    );
    assert_eq!(
        other_ids, subset,
        "case GetUsagePlanKeys(other plan): expected only its own associations {subset:?}, \
         got {other_ids:?}"
    );
}

#[tokio::test]
async fn get_usage_plan_keys_pages_through_the_added_order_on_position_and_limit() {
    let gateway = Gateway::new();
    let plan = bare_plan(&gateway, "key-collection-paging").await;
    let added = metered_table(&gateway, &plan).await;
    let total = added.len();

    // Rows span the documented page-size range, its bounds and one past each bound: a
    // size below 1 and a size above the maximum are clamped into the range rather than
    // refused.
    for limit in [
        0,
        1,
        2,
        3,
        total as i64,
        total as i64 + 1,
        DEFAULT_PAGE_LIMIT as i64,
        MAX_PAGE_LIMIT,
        MAX_PAGE_LIMIT + 1,
        10_000,
    ] {
        let case = format!("GetUsagePlanKeys(limit={limit})");
        let effective = usize::try_from(limit.clamp(1, MAX_PAGE_LIMIT)).unwrap_or(1);
        let mut seen: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &position {
                None => format!("limit={limit}"),
                Some(cursor) => format!("limit={limit}&position={cursor}"),
            };
            let body = listed(&gateway, &plan, &query).await;
            let page = listed_ids(&body, &case);
            assert!(
                page.len() <= effective,
                "case {case}: a page holds at most {effective} items, got {}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {case}: paging did not terminate after {pages} pages"
            );
            match body["position"].as_str() {
                None => break,
                Some(cursor) => position = Some(cursor.to_owned()),
            }
        }
        assert_eq!(
            seen, added,
            "case {case}: paging must reassemble the added order {added:?}, got {seen:?}"
        );
    }

    // Crossed with every cursor the sequence holds: a page resumes AT the cursor it is
    // given — the cursor a page hands back is the first id the page did NOT report — so
    // the page is the sequence from that id on, and a cursor is handed back only while the
    // sequence holds more.
    for (index, cursor) in added.iter().enumerate() {
        let case = format!("GetUsagePlanKeys(position={cursor}, limit=2)");
        let body = listed(&gateway, &plan, &format!("position={cursor}&limit=2")).await;
        let page = listed_ids(&body, &case);
        let start = index;
        let end = (start + 2).min(total);
        assert_eq!(
            page,
            added[start..end].to_vec(),
            "case {case}: expected {:?}, got {page:?}",
            &added[start..end]
        );
        let next = added.get(end).cloned();
        assert_eq!(
            body["position"].as_str().map(str::to_owned),
            next,
            "case {case}: expected the cursor {next:?}, got {:?}",
            body["position"]
        );
    }

    // A cursor naming nothing in the sequence is a bad request, not an empty page: one
    // never minted, one id-shaped, and a key id of ANOTHER plan's collection.
    let other = bare_plan(&gateway, "key-collection-paging-other").await;
    let stranger = string_member(
        &create_key(&gateway, "a key of another plan", json!({ "name": "stranger" })).await,
        "id",
        "a key of another plan",
    );
    meter_key(&gateway, &other, &stranger).await;
    for cursor in ["nosuchcursor", "0000000000", stranger.as_str()] {
        let answer = list_keys(&gateway, &plan, &format!("position={cursor}")).await;
        assert_failure(
            &answer,
            400,
            "BadRequestException",
            &format!("GetUsagePlanKeys(position={cursor:?})"),
        );
    }
}

#[tokio::test]
async fn name_query_narrows_the_collection_to_the_keys_whose_name_begins_with_it() {
    let gateway = Gateway::new();
    let plan = bare_plan(&gateway, "key-collection-name-query").await;

    // Rows differ along what the query generalises over: names sharing a prefix, a name
    // that IS another name's prefix, a name differing only in case, and a key with no
    // name at all.
    let names: Vec<Option<&str>> = vec![
        Some("alpha-one"),
        Some("alpha-two"),
        Some("alpha"),
        Some("Alpha-upper"),
        Some("beta-one"),
        None,
    ];
    let mut ids: Vec<String> = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let case = format!("CreateApiKey(name={name:?})");
        let request = match name {
            Some(name) => json!({ "name": name }),
            None => json!({ "description": format!("unnamed-{index}") }),
        };
        let key = create_key(&gateway, &case, request).await;
        let key_id = string_member(&key, "id", &case);
        meter_key(&gateway, &plan, &key_id).await;
        ids.push(key_id);
    }

    // Each query, with the indexes of the keys whose NAME BEGINS WITH it. The unnamed key
    // (index 5) is selected by no query, including the empty one.
    let cases: Vec<(&str, Vec<usize>)> = vec![
        ("alpha", vec![0, 1, 2]),
        ("alpha-", vec![0, 1]),
        ("alpha-one", vec![0]),
        ("alpha-one-more", Vec::new()),
        ("Alpha", vec![3]),
        ("beta", vec![4]),
        ("gamma", Vec::new()),
        ("", vec![0, 1, 2, 3, 4]),
    ];
    for (query, expected_indexes) in &cases {
        let case = format!("GetUsagePlanKeys(nameQuery={query:?})");
        let expected: Vec<String> = expected_indexes
            .iter()
            .map(|index| ids[*index].clone())
            .collect();
        let body = listed(&gateway, &plan, &format!("name={query}")).await;
        let listed_keys = listed_ids(&body, &case);
        assert_eq!(
            listed_keys, expected,
            "case {case}: expected the matching keys in added order {expected:?}, got \
             {listed_keys:?}"
        );

        // Paging walks the NARROWED sequence, not the whole collection.
        let mut seen: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        loop {
            let paged = match &position {
                None => format!("name={query}&limit=1"),
                Some(cursor) => format!("name={query}&limit=1&position={cursor}"),
            };
            let page_body = listed(&gateway, &plan, &paged).await;
            let page = listed_ids(&page_body, &case);
            assert!(
                page.len() <= 1,
                "case {case}: a limit=1 page holds at most one item, got {}",
                page.len()
            );
            seen.extend(page);
            match page_body["position"].as_str() {
                None => break,
                Some(cursor) => position = Some(cursor.to_owned()),
            }
            assert!(
                seen.len() <= expected.len(),
                "case {case}: paging the narrowed sequence must stop at {} items, got {seen:?}",
                expected.len()
            );
        }
        assert_eq!(
            seen, expected,
            "case {case}: paging the narrowed sequence must reassemble {expected:?}, got \
             {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_usage_plan_id_naming_no_stored_plan_is_refused() {
    let gateway = Gateway::new();
    let plan = bare_plan(&gateway, "key-collection-identifiers").await;
    let key = create_key(&gateway, "identifiers", json!({ "name": "identified-key" })).await;
    let key_id = string_member(&key, "id", "identifiers");
    meter_key(&gateway, &plan, &key_id).await;

    // A plan stored and then DELETED: its identifier was once a real one, which is the
    // row an id-shaped check would miss.
    let doomed = bare_plan(&gateway, "key-collection-doomed").await;
    let deleted = gateway
        .call("DELETE", &format!("/usageplans/{doomed}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteUsagePlan: expected the modelled 202, got {} with {:?}",
        deleted.status, deleted.body
    );

    // A near miss of the stored plan id: one character changed.
    let mut chars: Vec<char> = plan.chars().collect();
    chars[0] = if chars[0] == 'a' { 'b' } else { 'a' };
    let near_miss: String = chars.into_iter().collect();

    // Rows differ in the FORM the unresolvable identifier takes: never minted, id-shaped,
    // a near miss, the API key's id, and a deleted plan.
    for identifier in [
        "nosuchplan",
        "0000000000",
        near_miss.as_str(),
        key_id.as_str(),
        doomed.as_str(),
    ] {
        for query in ["", "limit=1", "name=identified-key"] {
            let case = format!("GetUsagePlanKeys({identifier:?}, {query:?})");
            let answer = list_keys(&gateway, identifier, query).await;
            assert_failure(&answer, 404, "NotFoundException", &case);
        }
    }

    // The stored plan IS served, so the refusals above are about the identifier and not
    // about the operation.
    let served = listed_ids(
        &listed(&gateway, &plan, "").await,
        "GetUsagePlanKeys(the stored plan)",
    );
    assert_eq!(
        served,
        vec![key_id.clone()],
        "case GetUsagePlanKeys(the stored plan): expected the one metered key, got {served:?}"
    );
}

#[tokio::test]
async fn the_collection_follows_the_stored_associations_and_a_read_changes_nothing() {
    let gateway = Gateway::new();
    let plan = bare_plan(&gateway, "key-collection-churn").await;
    let added = metered_table(&gateway, &plan).await;

    // Deleting an API key leaves no key for the plan to meter, so its association is on
    // no page — the collection follows the store rather than a copy taken at add time.
    // Rows: the first key, a middle one and the last, so the removal is asserted at each
    // end of the sequence and in the middle.
    let mut expected = added.clone();
    for index in [0, 2, expected.len() - 1] {
        let key_id = added[index].clone();
        let case = format!("DeleteApiKey({key_id})");
        let deleted = gateway
            .call("DELETE", &format!("/apikeys/{key_id}"), None)
            .await;
        assert_eq!(
            deleted.status, 202,
            "case {case}: expected the modelled 202, got {} with {:?}",
            deleted.status, deleted.body
        );
        expected.retain(|id| id != &key_id);
        let ids = listed_ids(
            &listed(&gateway, &plan, "").await,
            &format!("GetUsagePlanKeys(after {case})"),
        );
        assert_eq!(
            ids, expected,
            "case {case}: the collection must report the remaining associations {expected:?}, \
             got {ids:?}"
        );
    }

    // Re-adding a key puts it at the END of the sequence: the association's rank is the
    // one the add stamped on it.
    let late = string_member(
        &create_key(&gateway, "a late key", json!({ "name": "late-key" })).await,
        "id",
        "a late key",
    );
    meter_key(&gateway, &plan, &late).await;
    expected.push(late);
    let ids = listed_ids(
        &listed(&gateway, &plan, "").await,
        "GetUsagePlanKeys(after a late add)",
    );
    assert_eq!(
        ids, expected,
        "case GetUsagePlanKeys(after a late add): expected {expected:?}, got {ids:?}"
    );

    // Every shape of read this collection serves, none of which may change what a later
    // caller sees.
    let before = listed(&gateway, &plan, "").await;
    let queries = [
        String::new(),
        "limit=1".to_string(),
        format!("limit=2&position={}", expected[1]),
        "name=late".to_string(),
        "name=nosuchname".to_string(),
    ];
    for query in &queries {
        let _ = list_keys(&gateway, &plan, query).await;
    }
    for key_id in &expected {
        let _ = get_usage_plan_key(&gateway, &plan, key_id).await;
    }
    let after = listed(&gateway, &plan, "").await;
    assert_eq!(
        after, before,
        "case GetUsagePlanKeys(after reads): a read must change nothing, got {after:?}"
    );
}
