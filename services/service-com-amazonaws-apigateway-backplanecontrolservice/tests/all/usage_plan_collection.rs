// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetUsagePlans`, the account's usage plan collection.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The RULES under test, each over a SET of inputs rather than one recorded call:
//!
//! * the collection reports the account's STORED plans in creation order, each rendered
//!   exactly as the plan's own read reports it, over a table of plans spanning the
//!   optional members a plan may and may not carry;
//! * it reports one `position`/`limit` page of that sequence at a time, over page sizes
//!   across the documented range (default 25, maximum 500), each published bound and one
//!   past each, crossed with every cursor the sequence holds, and a cursor naming nothing
//!   in the sequence is a bad request rather than an empty page;
//! * `keyId` narrows the sequence to the plans that meter that API key, in the same
//!   order, and pages through the narrowed sequence;
//! * a read changes nothing a later caller sees.
//!
//! [`Gateway`], [`Answer`], [`assert_failure`], [`string_member`] and [`page_items`] are
//! this crate's one wire-test harness, reused rather than standing a second emulator up
//! another way; `create_usage_plan` / `get_usage_plan` and `create_key` are the usage plan
//! and API key suites' one spelling of those calls.

use serde_json::{json, Value};

use crate::api_key_lifecycle::create_key;
use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, string_member, Answer, Gateway,
};
use crate::stage_lifecycle::create_deployed_stage;
use crate::usage_plan_key_lifecycle::create_usage_plan_key;
use crate::usage_plan_lifecycle::{create_usage_plan, get_usage_plan};

/// The default page size API Gateway applies when a request names no `limit`, and the
/// largest it accepts — from `GetUsagePlansRequest$limit`'s own documentation, "The
/// default value is 25 and the maximum value is 500"
/// (`smithy_query(query="doc", shape="GetUsagePlansRequest", member="limit")`).
const DEFAULT_PAGE_LIMIT: usize = 25;
const MAX_PAGE_LIMIT: i64 = 500;

/// `GetUsagePlans` with `query` appended when it is not empty.
async fn list_plans(gateway: &Gateway, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/usageplans".to_string()
    } else {
        format!("/usageplans?{query}")
    };
    gateway.call("GET", &uri, None).await
}

/// `GetUsagePlans` with `query`, asserting the modelled 200 and returning the body.
async fn listed(gateway: &Gateway, query: &str) -> Value {
    let answer = list_plans(gateway, query).await;
    assert_eq!(
        answer.status, 200,
        "case GetUsagePlans({query:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// The `id` of every plan on a page, in page order.
fn listed_ids(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| string_member(item, "id", case))
        .collect()
}

/// `CreateUsagePlanKey` for an `API_KEY`, asserting the modelled 201. The call itself is
/// the usage plan key suite's one spelling of it (`API_KEY` is the one key type
/// `UsagePlanKey$type` declares valid).
async fn meter_key(gateway: &Gateway, plan: &str, key_id: &str) {
    let case = format!("CreateUsagePlanKey({plan}, {key_id})");
    let answer = create_usage_plan_key(gateway, plan, key_id, "API_KEY").await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// Create `requests` as usage plans, in order, answering the ids in creation order.
async fn created_plans(gateway: &Gateway, requests: &[(&str, Value)]) -> Vec<String> {
    let mut ids = Vec::new();
    for (case, request) in requests {
        let body = create_usage_plan(gateway, case, request.clone()).await;
        ids.push(string_member(&body, "id", case));
    }
    ids
}

/// A table of usage plan requests spanning the optional members a plan may carry: none of
/// them, each of them, and all at once. `api` and its deployed stages are what an
/// `apiStages` entry must name.
fn plan_table(api: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("name only", json!({ "name": "collection-plan-a" })),
        (
            "described",
            json!({ "name": "collection-plan-b", "description": "the second plan" }),
        ),
        (
            "throttled",
            json!({
                "name": "collection-plan-c",
                "throttle": { "burstLimit": 10, "rateLimit": 5.0 },
            }),
        ),
        (
            "quota",
            json!({
                "name": "collection-plan-d",
                "quota": { "limit": 1000, "offset": 0, "period": "MONTH" },
            }),
        ),
        (
            "one api stage",
            json!({
                "name": "collection-plan-e",
                "apiStages": [{ "apiId": api, "stage": "prod" }],
            }),
        ),
        (
            "every member",
            json!({
                "name": "collection-plan-f",
                "description": "the last plan",
                "apiStages": [
                    { "apiId": api, "stage": "prod" },
                    { "apiId": api, "stage": "beta" },
                ],
                "throttle": { "burstLimit": 20, "rateLimit": 9.5 },
                "quota": { "limit": 50, "offset": 1, "period": "WEEK" },
                "tags": { "team": "api", "env": "test" },
            }),
        ),
    ]
}

/// An API with two deployed stages, for the rows that associate api stages.
async fn api_with_stages(gateway: &Gateway, name: &str) -> String {
    let (api, _) = create_api(gateway, name).await;
    create_deployed_stage(gateway, &api, "prod").await;
    create_deployed_stage(gateway, &api, "beta").await;
    api
}

#[tokio::test]
async fn get_usage_plans_reports_every_stored_plan_as_its_own_read_does() {
    let gateway = Gateway::new();

    // An account with no usage plan has an EMPTY collection, not a missing one.
    let empty = listed(&gateway, "").await;
    assert!(
        listed_ids(&empty, "GetUsagePlans(no plans)").is_empty(),
        "case GetUsagePlans(no plans): expected no items, got {empty:?}"
    );
    assert!(
        empty["position"].is_null(),
        "case GetUsagePlans(no plans): the last page omits position, got {:?}",
        empty["position"]
    );

    let api = api_with_stages(&gateway, "usage-plan-collection").await;
    let table = plan_table(&api);
    let created = created_plans(&gateway, &table).await;

    let all = listed(&gateway, "").await;
    let ids = listed_ids(&all, "GetUsagePlans(all)");
    assert_eq!(
        ids, created,
        "case GetUsagePlans(all): expected the plans in creation order {created:?}, got {ids:?}"
    );

    // Each listed plan is the SAME record the plan's own read reports — the collection
    // renders the stored record, it does not build a second rendering of it.
    let items = page_items(&all, "GetUsagePlans(all)");
    for ((case, _), (id, item)) in table.iter().zip(created.iter().zip(items.iter())) {
        let read = get_usage_plan(&gateway, id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected GetUsagePlan 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            item, &read.body,
            "case {case}: the listed plan must equal its own read {:?}, got {item:?}",
            read.body
        );
    }

    // A listing with no `limit` reports the default page size at most.
    assert!(
        items.len() <= DEFAULT_PAGE_LIMIT,
        "case GetUsagePlans(all): a default page holds at most {DEFAULT_PAGE_LIMIT} items, got {}",
        items.len()
    );
}

#[tokio::test]
async fn get_usage_plans_pages_through_creation_order_on_position_and_limit() {
    let gateway = Gateway::new();
    let api = api_with_stages(&gateway, "usage-plan-paging").await;
    let created = created_plans(&gateway, &plan_table(&api)).await;
    let total = created.len();

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
        let case = format!("GetUsagePlans(limit={limit})");
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
            seen, created,
            "case {case}: paging must reassemble creation order {created:?}, got {seen:?}"
        );
    }

    // Crossed with every cursor the sequence holds: a page resumes AT the cursor it is
    // given — the cursor a page hands back is the first id the page did NOT report — so
    // the page is the sequence from that id on, and a cursor is handed back only while the
    // sequence holds more.
    for (index, cursor) in created.iter().enumerate() {
        let case = format!("GetUsagePlans(position={cursor}, limit=2)");
        let body = listed(&gateway, &format!("position={cursor}&limit=2")).await;
        let page = listed_ids(&body, &case);
        let start = index;
        let end = (start + 2).min(total);
        assert_eq!(
            page,
            created[start..end].to_vec(),
            "case {case}: expected {:?}, got {page:?}",
            &created[start..end]
        );
        let next = created.get(end).cloned();
        assert_eq!(
            body["position"].as_str().map(str::to_owned),
            next,
            "case {case}: expected the cursor {next:?}, got {:?}",
            body["position"]
        );
    }

    // A cursor naming nothing in the sequence is a bad request, not an empty page.
    for cursor in ["nosuchcursor", "0000000000", "collection-plan-a"] {
        let answer = list_plans(&gateway, &format!("position={cursor}")).await;
        assert_failure(
            &answer,
            400,
            "BadRequestException",
            &format!("GetUsagePlans(position={cursor:?})"),
        );
    }
}

#[tokio::test]
async fn key_id_narrows_the_collection_to_the_plans_that_meter_that_api_key() {
    let gateway = Gateway::new();
    let api = api_with_stages(&gateway, "usage-plan-key-filter").await;
    let created = created_plans(&gateway, &plan_table(&api)).await;

    // Three keys spanning the association counts the filter generalises over: a key
    // several plans meter, a key one plan meters, and a key no plan meters.
    let mut keys = Vec::new();
    for n in 0..3 {
        let body = create_key(
            &gateway,
            &format!("CreateApiKey(filter-{n})"),
            json!({ "name": format!("filter-key-{n}"), "enabled": true }),
        )
        .await;
        keys.push(string_member(&body, "id", "CreateApiKey(filter)"));
    }

    let metered: Vec<Vec<usize>> = vec![vec![0, 2, 5], vec![3], Vec::new()];
    for (key_index, plan_indexes) in metered.iter().enumerate() {
        for plan_index in plan_indexes {
            meter_key(&gateway, &created[*plan_index], &keys[key_index]).await;
        }
    }

    for (key_index, plan_indexes) in metered.iter().enumerate() {
        let key = &keys[key_index];
        let case = format!("GetUsagePlans(keyId={key})");
        let expected: Vec<String> = plan_indexes
            .iter()
            .map(|index| created[*index].clone())
            .collect();
        let body = listed(&gateway, &format!("keyId={key}")).await;
        let ids = listed_ids(&body, &case);
        assert_eq!(
            ids, expected,
            "case {case}: expected the metering plans in creation order {expected:?}, got {ids:?}"
        );

        // Paging walks the NARROWED sequence, not the whole collection.
        let mut seen: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        loop {
            let query = match &position {
                None => format!("keyId={key}&limit=1"),
                Some(cursor) => format!("keyId={key}&limit=1&position={cursor}"),
            };
            let page_body = listed(&gateway, &query).await;
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
                "case {case}: paging the narrowed sequence must stop at {} items, got {:?}",
                expected.len(),
                seen
            );
        }
        assert_eq!(
            seen, expected,
            "case {case}: paging the narrowed sequence must reassemble {expected:?}, got {seen:?}"
        );
    }

    // An id no stored API key answers to is metered by no plan, so it narrows the
    // collection to nothing.
    for unknown in ["nosuchkey", "0123456789"] {
        let case = format!("GetUsagePlans(keyId={unknown})");
        let body = listed(&gateway, &format!("keyId={unknown}")).await;
        let ids = listed_ids(&body, &case);
        assert!(
            ids.is_empty(),
            "case {case}: an unmetered key narrows the collection to nothing, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn listing_usage_plans_does_not_change_what_a_later_caller_sees() {
    let gateway = Gateway::new();
    let api = api_with_stages(&gateway, "usage-plan-collection-reads").await;
    let created = created_plans(&gateway, &plan_table(&api)).await;
    let key = string_member(
        &create_key(
            &gateway,
            "CreateApiKey(reads)",
            json!({ "name": "collection-read-key", "enabled": true }),
        )
        .await,
        "id",
        "CreateApiKey(reads)",
    );
    meter_key(&gateway, &created[1], &key).await;

    let before = listed(&gateway, "").await;
    // Every shape of read this collection serves: the whole sequence, a page of it, a
    // page from a cursor, a narrowed sequence, and one narrowed to nothing.
    let queries = [
        String::new(),
        "limit=1".to_string(),
        format!("limit=2&position={}", created[2]),
        format!("keyId={key}"),
        "keyId=nosuchkey".to_string(),
    ];
    for query in &queries {
        let _ = list_plans(&gateway, query).await;
    }
    for id in &created {
        let _ = get_usage_plan(&gateway, id).await;
    }
    let after = listed(&gateway, "").await;
    assert_eq!(
        after, before,
        "case GetUsagePlans(after reads): a read must change nothing, got {after:?}"
    );
}
