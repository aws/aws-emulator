// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetApiKeys`, the account's API key collection.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The RULES under test, each over a SET of inputs rather than one recorded call:
//!
//! * the collection reports the account's STORED keys in creation order — the stable
//!   sequence a `position` cursor walks — each rendered exactly as the key's own read
//!   reports it, over a table of keys spanning the optional members a key may and may not
//!   carry;
//! * that sequence is fixed when a key is created, so nothing a caller can change moves a
//!   key in it: renaming, describing, enabling or disabling a key leaves it where it was;
//! * it reports one `position`/`limit` page of that sequence at a time, over page sizes
//!   across the documented range (default 25, maximum 500), each published bound and one
//!   past each, crossed with every cursor the sequence holds, and a cursor naming nothing
//!   in the sequence is a bad request rather than an empty page;
//! * `includeValues` decides whether the page carries each key's secret value, exactly as
//!   `includeValue` does for a single key's read;
//! * `nameQuery` narrows the sequence to the keys whose name begins with it and
//!   `customerId` to the keys stamped with that customer, in the same order, and paging
//!   walks the narrowed sequence;
//! * a read changes nothing a later caller sees.
//!
//! [`Gateway`], [`Answer`], [`assert_failure`], [`string_member`] and [`page_items`] are
//! this crate's one wire-test harness, reused rather than standing a second emulator up
//! another way; `create_key` / `get_key` are the API key lifecycle suite's one spelling
//! of those calls.

use serde_json::{json, Value};

use crate::api_key_lifecycle::{create_key, get_key};
use crate::patch_updates::{patch_ok, replace};
use crate::rest_api_lifecycle::{assert_failure, page_items, string_member, Answer, Gateway};

/// The default page size API Gateway applies when a request names no `limit`, and the
/// largest it accepts — from `GetApiKeysRequest$limit`'s own documentation, "The default
/// value is 25 and the maximum value is 500"
/// (`smithy_query(query="doc", shape="GetApiKeysRequest", member="limit")`).
const DEFAULT_PAGE_LIMIT: usize = 25;
const MAX_PAGE_LIMIT: i64 = 500;

/// `GetApiKeys` with `query` appended when it is not empty.
async fn list_keys(gateway: &Gateway, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/apikeys".to_string()
    } else {
        format!("/apikeys?{query}")
    };
    gateway.call("GET", &uri, None).await
}

/// `GetApiKeys` with `query`, asserting the modelled 200 and returning the body.
async fn listed(gateway: &Gateway, query: &str) -> Value {
    let answer = list_keys(gateway, query).await;
    assert_eq!(
        answer.status, 200,
        "case GetApiKeys({query:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// The `id` of every key on a page, in page order.
fn listed_ids(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| string_member(item, "id", case))
        .collect()
}

/// A table of `CreateApiKey` requests spanning the optional members a key may carry: none
/// of them, each of them, several tags, a caller-supplied value, and the two customers and
/// the name prefixes the filter rows select on.
///
/// The rows are named in DESCENDING alphabetical order of creation on purpose, so the
/// sequence the collection reports is distinguishable from the keys' names: renaming them
/// into ascending order must not move any of them.
fn key_table() -> Vec<(&'static str, Value)> {
    vec![
        ("every member absent", json!({})),
        (
            "tagged",
            json!({ "name": "beta-two", "tags": { "team": "api", "env": "test" } }),
        ),
        (
            "a marketplace customer",
            json!({ "name": "beta-one", "enabled": true, "customerId": "customer-b" }),
        ),
        (
            "named and described, with the shared customer",
            json!({
                "name": "alpha-two",
                "description": "the second key",
                "customerId": "customer-a",
            }),
        ),
        (
            "a caller-supplied value",
            json!({
                "name": "alpha-three",
                "enabled": true,
                "value": "collection-supplied-value-01",
            }),
        ),
        (
            "the shared customer again",
            json!({ "name": "alpha-one", "customerId": "customer-a" }),
        ),
    ]
}

/// Create `requests` as API keys, in order, answering `(id, value)` in creation order —
/// which IS the order `GetApiKeys` reports them in, the sequence a `position` cursor walks.
async fn created_keys(gateway: &Gateway, requests: &[(&str, Value)]) -> Vec<(String, String)> {
    let mut created = Vec::new();
    for (case, request) in requests {
        let body = create_key(gateway, case, request.clone()).await;
        created.push((
            string_member(&body, "id", case),
            string_member(&body, "value", case),
        ));
    }
    created
}

#[tokio::test]
async fn get_api_keys_reports_every_stored_key_as_its_own_read_does() {
    let gateway = Gateway::new();

    // An account with no API key has an EMPTY collection, not a missing one.
    let empty = listed(&gateway, "").await;
    assert!(
        listed_ids(&empty, "GetApiKeys(no keys)").is_empty(),
        "case GetApiKeys(no keys): expected no items, got {empty:?}"
    );
    assert!(
        empty["position"].is_null(),
        "case GetApiKeys(no keys): the last page omits position, got {:?}",
        empty["position"]
    );
    assert!(
        empty["warnings"].is_null(),
        "case GetApiKeys(no keys): a listing imports nothing, so it reports no warnings, got {:?}",
        empty["warnings"]
    );

    let created = created_keys(&gateway, &key_table()).await;
    let creation_order: Vec<String> = created.iter().map(|(id, _)| id.clone()).collect();

    let all = listed(&gateway, "").await;
    let listed_order = listed_ids(&all, "GetApiKeys(all)");
    assert_eq!(
        listed_order, creation_order,
        "case GetApiKeys(all): expected the keys in creation order {creation_order:?}, got {listed_order:?}"
    );

    // Each listed key is the SAME record the key's own read reports — the collection
    // renders the stored record, it does not build a second rendering of it.
    let items = page_items(&all, "GetApiKeys(all)");
    for (id, item) in creation_order.iter().zip(items.iter()) {
        let case = format!("GetApiKeys(all) item {id}");
        let read = get_key(&gateway, id, false).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected GetApiKey 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            item, &read.body,
            "case {case}: the listed key must equal its own read {:?}, got {item:?}",
            read.body
        );
    }

    // A listing with no `limit` reports the default page size at most.
    assert!(
        items.len() <= DEFAULT_PAGE_LIMIT,
        "case GetApiKeys(all): a default page holds at most {DEFAULT_PAGE_LIMIT} items, got {}",
        items.len()
    );
}

#[tokio::test]
async fn the_collection_order_does_not_depend_on_what_a_caller_can_change() {
    let gateway = Gateway::new();
    let table = key_table();
    let created = created_keys(&gateway, &table).await;
    let creation_order: Vec<String> = created.iter().map(|(id, _)| id.clone()).collect();

    let before = listed_ids(&listed(&gateway, "").await, "GetApiKeys(before updates)");
    assert_eq!(
        before, creation_order,
        "case GetApiKeys(before updates): expected creation order {creation_order:?}, got {before:?}"
    );

    // The sequence is not keyed to the keys' NAMES: `key_table` names its rows in
    // descending alphabetical order of creation, and the first round below renames every
    // key into ascending order, so a collection ordered by name would come back reversed.
    let names: Vec<String> = table
        .iter()
        .filter_map(|(_, request)| {
            request
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    let mut ascending = names.clone();
    ascending.sort();
    assert_ne!(
        names, ascending,
        "the table's own names must not already be ascending, got {names:?}"
    );

    // Rows differ along the mutable members an update may move: the name the hypothetical
    // alphabetical order would key off, the description, and the enabled flag. After each
    // round the sequence is unchanged.
    let rounds: Vec<(&str, Vec<Value>)> = vec![
        ("renamed into ascending order", Vec::new()),
        (
            "described",
            vec![replace("/description", "updated by the collection suite")],
        ),
        ("disabled", vec![replace("/enabled", "false")]),
        ("enabled", vec![replace("/enabled", "true")]),
    ];

    for (round, operations) in &rounds {
        for (index, id) in creation_order.iter().enumerate() {
            let case = format!("UpdateApiKey({id}, {round})");
            let mut ops = operations.clone();
            if operations.is_empty() {
                // Rename every key so the names ASCEND in creation order — the mirror of
                // how the table created them.
                ops.push(replace("/name", &format!("renamed-{index:02}")));
            }
            patch_ok(&gateway, &case, &format!("/apikeys/{id}"), ops).await;
        }
        let after = listed_ids(&listed(&gateway, "").await, &format!("GetApiKeys({round})"));
        assert_eq!(
            after, creation_order,
            "case GetApiKeys({round}): an update must not move a key in the collection, expected {creation_order:?}, got {after:?}"
        );
    }
}

#[tokio::test]
async fn include_values_decides_whether_a_page_carries_each_key_value() {
    let gateway = Gateway::new();
    let created = created_keys(&gateway, &key_table()).await;
    let expected: Vec<String> = created.iter().map(|(id, _)| id.clone()).collect();
    let value_of: std::collections::BTreeMap<String, String> = created.into_iter().collect();

    // Rows span every spelling of the flag: absent, each boolean, and the flag crossed
    // with a narrowed page so the rule holds wherever the page came from.
    let cases: &[(&str, &str, bool)] = &[
        ("absent", "", false),
        ("false", "includeValues=false", false),
        ("true", "includeValues=true", true),
        ("true on a page", "includeValues=true&limit=2", true),
        ("false on a page", "includeValues=false&limit=2", false),
    ];

    for (case, query, with_values) in cases.iter().copied() {
        let case = format!("GetApiKeys(includeValues {case})");
        let body = listed(&gateway, query).await;
        let items = page_items(&body, &case);
        assert!(
            !items.is_empty(),
            "case {case}: expected a non-empty page, got {body:?}"
        );
        for (item, id) in items.iter().zip(expected.iter()) {
            assert_eq!(
                &string_member(item, "id", &case),
                id,
                "case {case}: expected the collection's own order, got {item:?}"
            );
            let value = &value_of[id];
            if with_values {
                assert_eq!(
                    item["value"].as_str(),
                    Some(value.as_str()),
                    "case {case}: expected the stored value {value:?}, got {:?}",
                    item["value"]
                );
            } else {
                assert!(
                    item["value"].is_null(),
                    "case {case}: a page that did not ask for values withholds them, got {:?}",
                    item["value"]
                );
            }
        }
    }
}

#[tokio::test]
async fn get_api_keys_pages_through_creation_order_on_position_and_limit() {
    let gateway = Gateway::new();
    let created = created_keys(&gateway, &key_table()).await;
    let ids: Vec<String> = created.iter().map(|(id, _)| id.clone()).collect();
    let total = ids.len();

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
        let case = format!("GetApiKeys(limit={limit})");
        let effective = usize::try_from(limit.clamp(1, MAX_PAGE_LIMIT)).unwrap_or(1);
        let mut seen: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &position {
                None => format!("limit={limit}"),
                Some(cursor) => format!("limit={limit}&position={cursor}"),
            };
            let body = listed(&gateway, &query).await;
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
            seen, ids,
            "case {case}: paging must reassemble the sequence {ids:?}, got {seen:?}"
        );
    }

    // Crossed with every cursor the sequence holds: a page resumes AT the cursor it is
    // given — the cursor a page hands back is the first id the page did NOT report — so
    // the page is the sequence from that id on, and a cursor is handed back only while the
    // sequence holds more.
    for (index, cursor) in ids.iter().enumerate() {
        let case = format!("GetApiKeys(position={cursor}, limit=2)");
        let body = listed(&gateway, &format!("position={cursor}&limit=2")).await;
        let page = listed_ids(&body, &case);
        let start = index;
        let end = (start + 2).min(total);
        assert_eq!(
            page,
            ids[start..end].to_vec(),
            "case {case}: expected {:?}, got {page:?}",
            &ids[start..end]
        );
        let next = ids.get(end).cloned();
        assert_eq!(
            body["position"].as_str().map(str::to_owned),
            next,
            "case {case}: expected the cursor {next:?}, got {:?}",
            body["position"]
        );
    }

    // A cursor naming nothing in the sequence is a bad request, not an empty page — and
    // a key's VALUE is not a cursor either: the sequence is a sequence of key ids.
    let value = created[0].1.clone();
    for cursor in ["nosuchcursor", "0000000000", value.as_str()] {
        let answer = list_keys(&gateway, &format!("position={cursor}")).await;
        assert_failure(
            &answer,
            400,
            "BadRequestException",
            &format!("GetApiKeys(position={cursor:?})"),
        );
    }
}

#[tokio::test]
async fn name_query_and_customer_id_narrow_the_collection_before_it_is_paged() {
    let gateway = Gateway::new();
    let created = created_keys(&gateway, &key_table()).await;
    let ids: Vec<String> = created.iter().map(|(id, _)| id.clone()).collect();

    // Rows are `(query, the indexes of key_table() it must select)`. They span the two
    // filters alone and together, a prefix that selects several keys, one that selects
    // exactly one, a full name, and queries that select nothing — the unnamed key at
    // index 0 is selected by no `nameQuery`, and no key carries a third customer.
    let rows: Vec<(String, Vec<usize>)> = vec![
        ("name=alpha".to_string(), vec![3, 4, 5]),
        ("name=alpha-t".to_string(), vec![3, 4]),
        ("name=alpha-tw".to_string(), vec![3]),
        ("name=alpha-one".to_string(), vec![5]),
        ("name=beta".to_string(), vec![1, 2]),
        ("name=zzz".to_string(), Vec::new()),
        ("customerId=customer-a".to_string(), vec![3, 5]),
        ("customerId=customer-b".to_string(), vec![2]),
        ("customerId=customer-c".to_string(), Vec::new()),
        ("name=alpha&customerId=customer-a".to_string(), vec![3, 5]),
        ("name=beta&customerId=customer-a".to_string(), Vec::new()),
        ("name=alpha-one&customerId=customer-a".to_string(), vec![5]),
    ];

    for (query, indexes) in &rows {
        let case = format!("GetApiKeys({query})");
        // The matching keys keep the collection's order, which is the creation order the
        // table built them in — the rows' indexes ascend, so this list does too.
        let expected: Vec<String> = indexes.iter().map(|index| ids[*index].clone()).collect();
        let body = listed(&gateway, query).await;
        let selected = listed_ids(&body, &case);
        assert_eq!(
            selected, expected,
            "case {case}: expected the matching keys in the collection's order {expected:?}, got {selected:?}"
        );

        // Paging walks the NARROWED sequence, not the whole collection.
        let mut seen: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        loop {
            let paged = match &position {
                None => format!("{query}&limit=1"),
                Some(cursor) => format!("{query}&limit=1&position={cursor}"),
            };
            let page_body = listed(&gateway, &paged).await;
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
            "case {case}: paging the narrowed sequence must reassemble {expected:?}, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn listing_api_keys_does_not_change_what_a_later_caller_sees() {
    let gateway = Gateway::new();
    let created = created_keys(&gateway, &key_table()).await;

    let before = listed(&gateway, "includeValues=true").await;
    // Every shape of read this collection serves: the whole sequence, a page of it, a
    // page from a cursor, the values asked for, a narrowed sequence, and one narrowed to
    // nothing.
    let queries = [
        String::new(),
        "limit=1".to_string(),
        format!("limit=2&position={}", created[2].0),
        "includeValues=true".to_string(),
        "name=alpha".to_string(),
        "customerId=customer-c".to_string(),
    ];
    for query in &queries {
        let _ = list_keys(&gateway, query).await;
    }
    for (id, _) in &created {
        let _ = get_key(&gateway, id, true).await;
    }
    let after = listed(&gateway, "includeValues=true").await;
    assert_eq!(
        after, before,
        "case GetApiKeys(after reads): a read must change nothing, got {after:?}"
    );
}
