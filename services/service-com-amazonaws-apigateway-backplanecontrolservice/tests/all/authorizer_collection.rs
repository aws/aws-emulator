// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetAuthorizers`: the paginated report of one API's authorizers.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The emulator, the request builder and the `CreateRestApi` / `CreateAuthorizer`
//! helpers are this crate's existing harness, reused from `rest_api_lifecycle` and
//! `authorizer_lifecycle`.
//!
//! The tests assert RULES over sets of inputs rather than one recorded call: that the
//! report is exactly the authorizers stored under the named API, in creation order, each
//! item member-for-member what `GetAuthorizer` reports for the same id (over a table of
//! collection sizes from empty to the per-API quota, spanning the modelled
//! `AuthorizerType` catalog); that `limit`/`position` walk that sequence exactly once for
//! every page size, including sizes outside the published window; that an authorizer
//! belongs to its own API; that an unresolvable cursor and an unknown API are refused;
//! and that listing changes nothing a later caller sees.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    DEFAULT_PAGE_LIMIT, MAX_AUTHORIZERS_PER_API, MAX_PAGE_LIMIT,
};

use crate::authorizer_lifecycle::{
    create_authorizer, created_authorizer, get_authorizer, lambda_uri, token_request,
    user_pool_arn,
};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, string_member, Answer, Gateway,
};

/// `GetAuthorizers` for `api` with `query` appended verbatim (empty for no query string).
async fn get_authorizers(gateway: &Gateway, api: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("/restapis/{api}/authorizers")
    } else {
        format!("/restapis/{api}/authorizers?{query}")
    };
    gateway.call("GET", &uri, None).await
}

/// `GetAuthorizers` for `api`, asserting the modelled 200 and answering the items of the
/// page.
async fn listed_authorizers(
    gateway: &Gateway,
    api: &str,
    query: &str,
    case: &str,
) -> (Vec<Value>, Option<String>) {
    let answer = get_authorizers(gateway, api, query).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetAuthorizers, got {} with {:?}",
        answer.status, answer.body
    );
    let next = answer.body["position"].as_str().map(str::to_owned);
    (page_items(&answer.body, case), next)
}

/// One authorizer request per `index`, rotating across the whole modelled
/// `AuthorizerType` catalog (`smithy_query(query="shape",
/// shape="com.amazonaws.apigateway#AuthorizerType")`: COGNITO_USER_POOLS | REQUEST |
/// TOKEN), each carrying the members its own type documents as required. So a collection
/// of any size holds authorizers of every type rather than copies of one.
fn authorizer_of_each_type(index: usize) -> Value {
    match index % 3 {
        0 => token_request(&format!("auth-{index}")),
        1 => json!({
            "name": format!("auth-{index}"),
            "type": "REQUEST",
            "authorizerUri": lambda_uri(&format!("request-{index}")),
            // A TTL of 0 disables caching, which is when a REQUEST authorizer needs no
            // identity source.
            "authorizerResultTtlInSeconds": 0,
        }),
        _ => json!({
            "name": format!("auth-{index}"),
            "type": "COGNITO_USER_POOLS",
            "identitySource": "method.request.header.Authorization",
            "providerARNs": [user_pool_arn(&format!("us-east-1_{index}"))],
        }),
    }
}

/// Create `count` authorizers on a fresh API, answering the API id and the ids in the
/// order they were created.
async fn api_with_authorizers(gateway: &Gateway, name: &str, count: usize) -> (String, Vec<String>) {
    let (api, _root) = create_api(gateway, name).await;
    let mut ids: Vec<String> = Vec::new();
    for index in 0..count {
        let case = format!("{name}: CreateAuthorizer(#{index})");
        let created =
            created_authorizer(gateway, &api, &case, authorizer_of_each_type(index)).await;
        ids.push(string_member(&created, "id", &case));
    }
    (api, ids)
}

#[tokio::test]
async fn get_authorizers_reports_exactly_the_stored_authorizers_in_creation_order() {
    // Rows differ along the size of the collection: none at all, one, a few, and the
    // whole per-API quota, each spanning the type catalog.
    let sizes = [0, 1, 2, 3, MAX_AUTHORIZERS_PER_API];

    let gateway = Gateway::new();
    for count in sizes {
        let case = format!("GetAuthorizers over {count} authorizer(s)");
        let (api, ids) =
            api_with_authorizers(&gateway, &format!("authorizer-listing-{count}"), count).await;

        // `limit=500` is the published maximum page size, so one call holds the whole
        // collection (the quota is well under it).
        let (items, next) = listed_authorizers(
            &gateway,
            &api,
            &format!("limit={MAX_PAGE_LIMIT}"),
            &case,
        )
        .await;
        assert_eq!(
            next, None,
            "case {case}: a page that holds the whole collection reports no cursor, got \
             {next:?}"
        );
        let listed: Vec<String> = items
            .iter()
            .map(|item| string_member(item, "id", &case))
            .collect();
        assert_eq!(
            listed, ids,
            "case {case}: expected the stored ids in creation order {ids:?}, got {listed:?}"
        );

        // Each listed item is the record a direct read reports, member for member: one
        // rendering of the `Authorizer` shape, not two.
        for (index, item) in items.iter().enumerate() {
            let id = &ids[index];
            let read = get_authorizer(&gateway, &api, id).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected 200 from GetAuthorizer({id}), got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                item, &read.body,
                "case {case}: listed item #{index} must equal GetAuthorizer({id}), \
                 got {item:?} against {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn limit_and_position_walk_the_collection_exactly_once() {
    // Rows differ along the page size: one item at a time, sizes that do and do not
    // divide the collection, the documented default, the published maximum — and sizes
    // OUTSIDE the published window, which are served rather than refused and must still
    // walk the sequence exactly once.
    let quota = i32::try_from(MAX_AUTHORIZERS_PER_API).expect("the quota fits an i32");
    let limits = [
        1,
        2,
        3,
        quota - 1,
        quota,
        quota + 1,
        DEFAULT_PAGE_LIMIT,
        MAX_PAGE_LIMIT,
        MAX_PAGE_LIMIT + 1,
        0,
        -7,
    ];

    let gateway = Gateway::new();
    let (api, expected) =
        api_with_authorizers(&gateway, "authorizer-paging", MAX_AUTHORIZERS_PER_API).await;

    for limit in limits {
        let case = format!("GetAuthorizers(limit={limit})");
        let mut seen: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        // The walk is bounded: a page that reports a cursor must have reported at least
        // one item, so the collection's size bounds the number of pages.
        for page in 0..=expected.len() {
            let query = match &position {
                None => format!("limit={limit}"),
                Some(cursor) => format!("limit={limit}&position={cursor}"),
            };
            let (items, next) = listed_authorizers(&gateway, &api, &query, &case).await;
            if items.is_empty() {
                // The page that holds nothing is where the collection ENDS: a cursor is
                // the last element a page handed out, so the walk is told it is over by
                // the empty page that reports no cursor of its own.
                assert!(
                    next.is_none(),
                    "case {case}: page {page} holds nothing, so it reports no cursor, got \
                     {next:?}"
                );
                break;
            }
            if limit > 0 {
                assert!(
                    items.len() <= usize::try_from(limit).unwrap_or(0),
                    "case {case}: page {page} holds at most {limit} items, got {}",
                    items.len()
                );
            }
            seen.extend(items.iter().map(|item| string_member(item, "id", &case)));
            match next {
                None => break,
                Some(cursor) => {
                    assert!(
                        expected.contains(&cursor),
                        "case {case}: the next position must name a stored authorizer, \
                         got {cursor:?}"
                    );
                    position = Some(cursor);
                }
            }
        }
        assert_eq!(
            seen, expected,
            "case {case}: paging must report creation order {expected:?} exactly once, \
             got {seen:?}"
        );
    }
}

#[tokio::test]
async fn an_authorizer_is_reported_only_through_the_api_that_holds_it() {
    let gateway = Gateway::new();
    let (first, first_ids) = api_with_authorizers(&gateway, "authorizer-listing-own", 3).await;
    let (second, second_ids) = api_with_authorizers(&gateway, "authorizer-listing-other", 2).await;

    for (case, api, expected) in [
        ("the first API", &first, &first_ids),
        ("the second API", &second, &second_ids),
    ] {
        let (items, _) = listed_authorizers(&gateway, api, "limit=500", case).await;
        let listed: Vec<String> = items
            .iter()
            .map(|item| string_member(item, "id", case))
            .collect();
        assert_eq!(
            &listed, expected,
            "case {case}: an API reports only its own authorizers, expected {expected:?}, \
             got {listed:?}"
        );
    }

    // An unknown `restApiId` resolves to no API at all, which is a 404 — not an empty
    // page.
    assert_failure(
        &get_authorizers(&gateway, "zzzzzzzzzz", "").await,
        404,
        "NotFoundException",
        "GetAuthorizers on an unknown API",
    );

    // The authorizers of a deleted API go with it, and so does its collection.
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{first}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &get_authorizers(&gateway, &first, "").await,
        404,
        "NotFoundException",
        "GetAuthorizers after DeleteRestApi",
    );
}

#[tokio::test]
async fn a_position_naming_nothing_in_this_apis_sequence_is_refused() {
    let gateway = Gateway::new();
    let (api, ids) = api_with_authorizers(&gateway, "authorizer-cursor", 3).await;
    let (other, other_ids) = api_with_authorizers(&gateway, "authorizer-cursor-other", 1).await;

    // Rows differ along the form of the cursor: one of an id's own form that no
    // authorizer carries, one that is well formed but names an authorizer of ANOTHER
    // API, one of a form no id takes, and one far longer than an id.
    for (case, cursor) in [
        ("a cursor no authorizer carries", "zzzzzzzzzz".to_string()),
        ("a cursor from another API", other_ids[0].clone()),
        ("a cursor of a form no id takes", "NOT-AN-ID".to_string()),
        ("a cursor far longer than an id", "q".repeat(64)),
    ] {
        assert_failure(
            &get_authorizers(&gateway, &api, &format!("position={cursor}")).await,
            400,
            "BadRequestException",
            case,
        );
    }

    // A cursor this API's own sequence carries is served, and resumes the page AT it.
    for (index, id) in ids.iter().enumerate() {
        let case = format!("GetAuthorizers(position={id})");
        let (items, _) =
            listed_authorizers(&gateway, &api, &format!("position={id}&limit=500"), &case).await;
        let listed: Vec<String> = items
            .iter()
            .map(|item| string_member(item, "id", &case))
            .collect();
        assert_eq!(
            listed,
            ids[index..].to_vec(),
            "case {case}: the page resumes at the cursor, expected {:?}, got {listed:?}",
            &ids[index..]
        );
    }

    // The refused cursors took nothing away: the other API still reports its own record.
    let (items, _) = listed_authorizers(&gateway, &other, "limit=500", "the other API").await;
    assert_eq!(
        items.len(),
        other_ids.len(),
        "case the other API: expected {} authorizer(s), got {items:?}",
        other_ids.len()
    );
}

#[tokio::test]
async fn listing_authorizers_changes_nothing_a_later_caller_sees() {
    let gateway = Gateway::new();
    let (api, ids) = api_with_authorizers(&gateway, "authorizer-listing-stable", 4).await;

    let mut rounds: Vec<Vec<Value>> = Vec::new();
    for round in 0..3 {
        let case = format!("GetAuthorizers(round {round})");
        let (items, _) = listed_authorizers(&gateway, &api, "limit=500", &case).await;
        rounds.push(items);
    }
    for (round, pair) in rounds.windows(2).enumerate() {
        assert_eq!(
            pair[0], pair[1],
            "case GetAuthorizers round {round} then {}: a report must not change the records, \
             got {:?} then {:?}",
            round + 1,
            pair[0],
            pair[1]
        );
    }

    // And a read of one record after the listings is still the record that was listed.
    for (index, id) in ids.iter().enumerate() {
        let case = format!("GetAuthorizer({id}) after listing");
        let read = get_authorizer(&gateway, &api, id).await;
        assert_eq!(
            read.body, rounds[0][index],
            "case {case}: expected the listed record {:?}, got {:?}",
            rounds[0][index], read.body
        );
    }

    // A listing consumes no quota either: the API is still full, not over.
    for index in ids.len()..MAX_AUTHORIZERS_PER_API {
        let case = format!("CreateAuthorizer(#{index}) after listing");
        created_authorizer(&gateway, &api, &case, authorizer_of_each_type(index)).await;
    }
    assert_failure(
        &create_authorizer(&gateway, &api, token_request("one-too-many")).await,
        429,
        "LimitExceededException",
        "CreateAuthorizer past the quota after listing",
    );
}
