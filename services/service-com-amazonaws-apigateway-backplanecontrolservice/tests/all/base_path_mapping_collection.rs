// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the base path mapping COLLECTION of a custom domain name:
//! `GetBasePathMappings`, read against `CreateBasePathMapping`, `GetBasePathMapping`,
//! `UpdateBasePathMapping` and `DeleteRestApi`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The RULES under test, each over a SET of inputs rather than one recorded call:
//!
//! * `GetBasePathMappings` reports the stored mappings of the ONE custom domain name the
//!   request's `domainName` + `domainNameId` pair resolves to — the same pair that
//!   record's own readers resolve it by — in creation order, one `position` / `limit` page
//!   at a time, each item rendered exactly as that mapping's own `GetBasePathMapping` read
//!   renders it;
//! * page sizes across the documented range (default 25, maximum 500), each published
//!   bound and one past each, crossed with every cursor the sequence holds, and a cursor
//!   naming no mapping of the collection is the modelled `BadRequestException`;
//! * a pair that resolves to no stored domain name is that record's missing-resource
//!   `NotFoundException` rather than an empty page, while a stored record carrying no
//!   mapping reports an EMPTY page;
//! * the listing is computed from the stored records, so a mapping a later call patches is
//!   reported as the store then holds it — and a delete of the REST API it names is refused
//!   while the mapping stands, so the listing never reports a mapping whose API has gone;
//!   and reading the collection changes nothing a later caller sees.
//!
//! [`Gateway`], [`Answer`], [`assert_failure`], [`string_member`] and [`page_items`] are
//! this crate's one wire-test harness, reused rather than standing a second emulator up
//! another way; `created_domain` is the domain-name suite's one spelling of that create
//! and `create_mapping` / `get_mapping` / `mapping_of` / `assert_refusal` /
//! `deployed_api` are the `base_path_mapping` suite's.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    NONE_BASE_PATH, NO_SUCH_DOMAIN_NAME, PRIVATE_ENDPOINT_TYPE,
};

use crate::base_path_mapping::{
    assert_refusal, create_mapping, deployed_api, get_mapping, mapping_of,
};
use crate::domain_name_collection::created_domain;
use crate::patch_updates::{patch, replace};
use crate::rest_api_lifecycle::{assert_failure, page_items, string_member, Answer, Gateway};

const BAD_REQUEST: &str = "BadRequestException";
const NOT_FOUND: (u16, &str) = (404, "NotFoundException");

/// `GetBasePathMappings` for `domain_name`, with `query` appended when it is not empty.
async fn list_mappings(gateway: &Gateway, domain_name: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("/domainnames/{domain_name}/basepathmappings")
    } else {
        format!("/domainnames/{domain_name}/basepathmappings?{query}")
    };
    gateway.call("GET", &uri, None).await
}

/// The `basePath` of every mapping on a page, in page order.
fn listed_base_paths(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| string_member(item, "basePath", case))
        .collect()
}

/// The page `query` reports for `domain_name`, asserting the modelled 200.
async fn page_of(gateway: &Gateway, domain_name: &str, query: &str, case: &str) -> Value {
    let answer = list_mappings(gateway, domain_name, query).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetBasePathMappings, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// Create one mapping on `domain_name` from `body`, asserting the modelled 201, and
/// answer the `basePath` the record was stored under.
async fn created_mapping(gateway: &Gateway, domain_name: &str, body: Value, case: &str) -> String {
    let created = create_mapping(gateway, domain_name, body).await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected 201 from CreateBasePathMapping, got {} with {:?}",
        created.status, created.body
    );
    string_member(&created.body, "basePath", case)
}

#[tokio::test]
async fn get_base_path_mappings_pages_through_creation_order_on_position_and_limit() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "collection-api", "prod").await;
    let domain_name = "collection.example.com";
    created_domain(&gateway, domain_name, json!({})).await;

    // A domain name that carries no mapping reports NO page member rather than a refusal
    // or an empty list: there is no collection to report a page of, and `{}` is the whole
    // answer a client reads.
    let empty = page_of(&gateway, domain_name, "", "the empty collection").await;
    assert_eq!(
        empty.get("item"),
        None,
        "case the empty collection: expected no page member, got {empty:?}"
    );
    assert_eq!(
        listed_base_paths(&empty, "the empty collection"),
        Vec::<String>::new(),
        "case the empty collection: expected no items, got {empty:?}"
    );
    assert!(
        empty["position"].is_null(),
        "case the empty collection: a single page omits position, got {:?}",
        empty["position"]
    );

    // The creates differ along what the rule generalises over: the SPELLING of the base
    // path (a plain one, one with digits, hyphens and underscores, the documented `(none)`
    // a request spells by naming none at all, one that extends another, one sorting before
    // every other) and whether the mapping names a stage — and the creation order is
    // deliberately NOT the base-path order the store's map holds them in.
    let creates: Vec<(&str, Value)> = vec![
        (
            "a plain base path",
            json!({ "basePath": "v2", "restApiId": api, "stage": stage }),
        ),
        (
            "no basePath at all",
            json!({ "restApiId": api, "stage": stage }),
        ),
        (
            "a base path with digits, hyphens and underscores",
            json!({ "basePath": "Orders-2_beta", "restApiId": api, "stage": stage }),
        ),
        (
            "a base path that names no stage",
            json!({ "basePath": "stageless", "restApiId": api }),
        ),
        (
            "a base path sorting before the others",
            json!({ "basePath": "alpha", "restApiId": api, "stage": stage }),
        ),
        (
            "a base path extending another",
            json!({ "basePath": "v2x", "restApiId": api, "stage": stage }),
        ),
        (
            "a base path naming the (none) stage",
            json!({ "basePath": "v1", "restApiId": api, "stage": NONE_BASE_PATH }),
        ),
    ];
    let mut created: Vec<String> = Vec::new();
    for (case, body) in creates {
        created.push(created_mapping(&gateway, domain_name, body, case).await);
    }
    assert_eq!(
        created.len(),
        7,
        "case the created collection: expected every create stored, got {created:?}"
    );

    // Creation order, which is not the base-path order: a listing that reported the store's
    // key order would differ here.
    let mut sorted = created.clone();
    sorted.sort();
    assert_ne!(
        created, sorted,
        "case the created collection: the table must not be created in base-path order, got {created:?}"
    );

    let all = page_of(&gateway, domain_name, "", "GetBasePathMappings(no limit)").await;
    assert_eq!(
        listed_base_paths(&all, "GetBasePathMappings(no limit)"),
        created,
        "case GetBasePathMappings(no limit): expected creation order {created:?}, got {all:?}"
    );
    assert_eq!(
        all["position"].as_str(),
        None,
        "case GetBasePathMappings(no limit): a page that holds the whole collection reports \
         no cursor, got {:?}",
        all["position"]
    );

    // Every listed mapping is the record its own reader reports, member for member: one
    // rendering of the `BasePathMapping` shape, whichever operation answers it.
    for item in page_items(&all, "GetBasePathMappings(no limit)") {
        let base_path = string_member(&item, "basePath", "a listed mapping");
        let case = format!("the listed mapping {base_path}");
        let read = get_mapping(&gateway, domain_name, &base_path).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetBasePathMapping, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            mapping_of(&item, &case),
            mapping_of(&read.body, &case),
            "case {case}: expected the listed record to match its own read, got {item:?} and {:?}",
            read.body
        );
    }

    // Rows span the documented page-size range (default 25, maximum 500), each published
    // bound and one past each — a size below 1 and a size above 500 are clamped into the
    // range rather than refused — crossed with every cursor the sequence holds.
    for limit in [0, 1, 2, 3, 7, 8, 25, 500, 501, 10_000] {
        let effective = usize::try_from(limit.clamp(1, 500)).expect("the clamped limit is positive");
        for (index, cursor) in std::iter::once(None)
            .chain(created.iter().map(Some))
            .enumerate()
        {
            // No cursor starts at the first record; a cursor is the base path of the
            // first record the previous page did NOT hand out — the key a mapping is
            // stored and addressed under — so its page resumes AT that record.
            let start = index.saturating_sub(1);
            let expected: Vec<String> = created
                .iter()
                .skip(start)
                .take(effective)
                .cloned()
                .collect();
            let query = match cursor {
                None => format!("limit={limit}"),
                Some(base_path) => format!("limit={limit}&position={base_path}"),
            };
            let case = format!("GetBasePathMappings({query})");
            let body = page_of(&gateway, domain_name, &query, &case).await;
            assert_eq!(
                listed_base_paths(&body, &case),
                expected,
                "case {case}: expected the page {expected:?}, got {body:?}"
            );
            // The cursor a page hands back names the first record it did NOT report, and
            // a page the collection holds nothing after hands back none: that is where the
            // collection ends.
            let consumed = start.saturating_add(expected.len());
            let handed_back = created.get(consumed).map(String::as_str);
            assert_eq!(
                body["position"].as_str(),
                handed_back,
                "case {case}: expected the cursor {handed_back:?}, got {:?}",
                body["position"]
            );
        }
    }

    // Paging with each size reassembles the whole collection, once each, in order.
    for limit in [1, 2, 3, 7, 25] {
        let case = format!("paging with limit={limit}");
        let mut seen: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &position {
                None => format!("limit={limit}"),
                Some(cursor) => format!("limit={limit}&position={cursor}"),
            };
            let body = page_of(&gateway, domain_name, &query, &case).await;
            seen.extend(listed_base_paths(&body, &case));
            pages += 1;
            assert!(pages <= 20, "case {case}: pagination did not terminate");
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

    // A cursor naming nothing in the sequence is a bad request, not an empty page —
    // including one that names a mapping of ANOTHER domain name.
    created_domain(&gateway, "elsewhere.example.com", json!({})).await;
    let elsewhere = created_mapping(
        &gateway,
        "elsewhere.example.com",
        json!({ "basePath": "foreign", "restApiId": api, "stage": stage }),
        "the mapping of another domain name",
    )
    .await;
    for cursor in ["nosuchpath", "V2", "", &elsewhere] {
        let answer = list_mappings(&gateway, domain_name, &format!("position={cursor}")).await;
        assert_failure(
            &answer,
            400,
            BAD_REQUEST,
            &format!("GetBasePathMappings(position={cursor:?})"),
        );
    }

    // Reading the collection changes nothing a later caller sees.
    let again = page_of(&gateway, domain_name, "", "GetBasePathMappings repeated").await;
    assert_eq!(
        page_items(&again, "GetBasePathMappings repeated"),
        page_items(&all, "GetBasePathMappings(no limit)"),
        "case GetBasePathMappings repeated: a read must not change the collection, got {again:?} after {all:?}"
    );
}

#[tokio::test]
async fn get_base_path_mappings_reports_the_mappings_of_the_record_the_pair_resolves_to() {
    let gateway = Gateway::new();
    let (first_api, first_stage) = deployed_api(&gateway, "pair-first", "prod").await;
    let (second_api, second_stage) = deployed_api(&gateway, "pair-second", "beta").await;

    // TWO private records carrying ONE name, told apart by their ids: a `domainNameId` is
    // supported only for a private record, and AWS lets several carry one name, so the pair
    // rather than the name selects the collection.
    let shared_name = "pairs.example.com";
    let mut ids: Vec<String> = Vec::new();
    for case in ["the first private record", "its same-named sibling"] {
        let created = created_domain(
            &gateway,
            shared_name,
            json!({ "endpointConfiguration": { "types": [PRIVATE_ENDPOINT_TYPE] } }),
        )
        .await;
        ids.push(string_member(&created, "domainNameId", case));
    }
    assert_ne!(
        ids[0], ids[1],
        "case two private records of one name: expected distinct domainNameIds, got {:?} twice",
        ids[0]
    );

    // A mapping under each record, pointing at its own API and stage, so a mapping listed
    // under the wrong record is visible. The URI carries the id as its query parameter,
    // which is how both this listing and the mapping's own reader address the record.
    let rows = [
        (
            "the first private record",
            ids[0].clone(),
            &first_api,
            &first_stage,
            "v1",
        ),
        (
            "its same-named sibling",
            ids[1].clone(),
            &second_api,
            &second_stage,
            "v2",
        ),
    ];
    for (case, id, api, stage, base_path) in &rows {
        let created = gateway
            .call(
                "POST",
                &format!("/domainnames/{shared_name}/basepathmappings?domainNameId={id}"),
                Some(json!({ "basePath": base_path, "restApiId": api, "stage": stage })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateBasePathMapping, got {} with {:?}",
            created.status, created.body
        );
    }

    // Each record lists its OWN mapping only, and a second listing reports the same page.
    for (case, id, api, stage, base_path) in &rows {
        let query = format!("domainNameId={id}");
        let body = page_of(&gateway, shared_name, &query, case).await;
        let items = page_items(&body, case);
        assert_eq!(
            items.len(),
            1,
            "case {case}: expected this record's one mapping, got {body:?}"
        );
        assert_eq!(
            mapping_of(&items[0], case),
            (
                (*base_path).to_string(),
                (*api).clone(),
                Some((*stage).clone())
            ),
            "case {case}: expected the mapping of this record only, got {:?}",
            items[0]
        );
        let repeated = page_of(&gateway, shared_name, &query, case).await;
        assert_eq!(
            page_items(&repeated, case),
            items,
            "case {case}: a read must not change the collection, got {repeated:?}"
        );
    }

    // Every pair here resolves to NO stored domain name, however near it is to one that
    // does: the private name without an id (the name alone selects no private record), with
    // an id no record carries, with one of its ids extended by a character, and an unknown
    // name carrying a stored id. Each is the domain name's own missing-resource refusal
    // rather than an empty page.
    for (case, domain_name, query) in [
        (
            "the private name with no domainNameId",
            shared_name.to_string(),
            String::new(),
        ),
        (
            "the private name with an unknown domainNameId",
            shared_name.to_string(),
            "domainNameId=zzzzz99999".to_string(),
        ),
        (
            "the private name with a stored id extended",
            shared_name.to_string(),
            format!("domainNameId={}x", ids[0]),
        ),
        (
            "an unknown name carrying a stored id",
            "absent.example.com".to_string(),
            format!("domainNameId={}", ids[0]),
        ),
        (
            "an unknown name with no id",
            "absent.example.com".to_string(),
            String::new(),
        ),
    ] {
        let answer = list_mappings(&gateway, &domain_name, &query).await;
        assert_refusal(&answer, NOT_FOUND, NO_SUCH_DOMAIN_NAME, case);
    }
}

#[tokio::test]
async fn the_listing_reports_the_stored_mappings_as_later_calls_leave_them() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "listing-api", "prod").await;
    let (other_api, other_stage) = deployed_api(&gateway, "listing-other-api", "beta").await;
    let domain_name = "mutating.example.com";
    created_domain(&gateway, domain_name, json!({})).await;

    for (case, body) in [
        (
            "the patched mapping",
            json!({ "basePath": "patched", "restApiId": api, "stage": stage }),
        ),
        (
            "the orphaned mapping",
            json!({ "basePath": "orphan", "restApiId": other_api, "stage": other_stage }),
        ),
    ] {
        created_mapping(&gateway, domain_name, body, case).await;
    }

    // A patch to one mapping is reported by the listing, because the page is cut out of the
    // stored records rather than out of what the create answered.
    let patched = patch(
        &gateway,
        &format!("/domainnames/{domain_name}/basepathmappings/patched"),
        vec![replace("/stage", NONE_BASE_PATH)],
    )
    .await;
    assert_eq!(
        patched.status, 200,
        "case the patched mapping: expected 200 from UpdateBasePathMapping, got {} with {:?}",
        patched.status, patched.body
    );

    // Deleting the REST API a mapping points at is REFUSED while the mapping stands — the
    // mapping names that API, so the delete would leave it pointing at an API that does
    // not exist — and the collection still reports the record the refusal protected.
    let refused = gateway
        .call("DELETE", &format!("/restapis/{other_api}"), None)
        .await;
    assert_eq!(
        refused.status, 400,
        "case the mapped api: expected 400 from DeleteRestApi while a mapping names it, got \
         {} with {:?}",
        refused.status, refused.body
    );

    let body = page_of(&gateway, domain_name, "", "after the later calls").await;
    let items = page_items(&body, "after the later calls");
    assert_eq!(
        items
            .iter()
            .map(|item| mapping_of(item, "after the later calls"))
            .collect::<Vec<_>>(),
        vec![
            (
                "patched".to_string(),
                api.clone(),
                Some(NONE_BASE_PATH.to_string())
            ),
            (
                "orphan".to_string(),
                other_api.clone(),
                Some(other_stage.clone())
            ),
        ],
        "case after the later calls: expected the stored records in creation order, got {body:?}"
    );

    // Each listed record still matches its own reader, the patched one and the mapped one
    // alike.
    for base_path in ["patched", "orphan"] {
        let case = format!("the listed mapping {base_path}");
        let read = get_mapping(&gateway, domain_name, base_path).await;
        let listed = items
            .iter()
            .find(|item| item["basePath"].as_str() == Some(base_path))
            .unwrap_or_else(|| panic!("case {case}: expected {base_path} listed, got {body:?}"));
        assert_eq!(
            mapping_of(listed, &case),
            mapping_of(&read.body, &case),
            "case {case}: expected the listed record to match its own read, got {listed:?} and {:?}",
            read.body
        );
    }
}
