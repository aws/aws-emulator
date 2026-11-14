// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetRequestValidators`: the paginated report of one API's request
//! validators.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The emulator, the request builder and the `CreateRestApi` /
//! `CreateRequestValidator` helpers are this crate's existing harness, reused from
//! `rest_api_lifecycle` and `request_validator_lifecycle`.
//!
//! The tests assert RULES over sets of inputs rather than one recorded call: that the
//! report is exactly the validators stored under the named API, in creation order, each
//! item member-for-member what `GetRequestValidator` reports for the same id (over a table
//! of collection sizes from empty to past the default page size, spanning every
//! combination of the two modelled validation flags and whether the request names the
//! validator); that `limit`/`position` walk that sequence exactly once for every page
//! size, including sizes outside the published window; that a validator belongs to its own
//! API; that an unresolvable cursor and an unknown API are refused; and that listing
//! changes nothing a later caller sees.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT,
};

use crate::request_validator_lifecycle::{created_validator, get_validator};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, string_member, Answer, Gateway,
};

/// `GetRequestValidators` for `api` with `query` appended verbatim (empty for no query
/// string).
async fn get_request_validators(gateway: &Gateway, api: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("/restapis/{api}/requestvalidators")
    } else {
        format!("/restapis/{api}/requestvalidators?{query}")
    };
    gateway.call("GET", &uri, None).await
}

/// `GetRequestValidators` for `api`, asserting the modelled 200 and answering the items of
/// the page beside the cursor it reports for the next one.
async fn listed_validators(
    gateway: &Gateway,
    api: &str,
    query: &str,
    case: &str,
) -> (Vec<Value>, Option<String>) {
    let answer = get_request_validators(gateway, api, query).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetRequestValidators, got {} with {:?}",
        answer.status, answer.body
    );
    let next = answer.body["position"].as_str().map(str::to_owned);
    (page_items(&answer.body, case), next)
}

/// One `CreateRequestValidator` request per `index`, rotating over every combination of
/// the two modelled flags (`validateRequestBody` and `validateRequestParameters`, each
/// `@default false` per `smithy_query(query="operation",
/// operation="CreateRequestValidator")`) and over whether the request names the validator
/// at all (`name` is optional). So a collection of any size holds validators of every
/// shape rather than copies of one.
fn validator_of_each_shape(index: usize) -> Value {
    let body = index.is_multiple_of(2);
    let parameters = index.is_multiple_of(3);
    // Every fifth request leaves `name` out, which a stored validator reports as absent.
    if index % 5 == 4 {
        json!({
            "validateRequestBody": body,
            "validateRequestParameters": parameters,
        })
    } else {
        json!({
            "name": format!("validator-{index}"),
            "validateRequestBody": body,
            "validateRequestParameters": parameters,
        })
    }
}

/// Create `count` request validators on a fresh API, answering the API id and the ids in
/// the order they were created.
async fn api_with_validators(gateway: &Gateway, name: &str, count: usize) -> (String, Vec<String>) {
    let (api, _root) = create_api(gateway, name).await;
    let mut ids: Vec<String> = Vec::new();
    for index in 0..count {
        let case = format!("{name}: CreateRequestValidator(#{index})");
        let created = created_validator(gateway, &api, &case, validator_of_each_shape(index)).await;
        ids.push(string_member(&created, "id", &case));
    }
    (api, ids)
}

/// A collection size past the documented default page size, so a listing that ignored
/// `limit` could not pass the tests below by accident.
const PAST_DEFAULT_PAGE: usize = DEFAULT_PAGE_LIMIT as usize + 1;

#[tokio::test]
async fn get_request_validators_reports_exactly_the_stored_validators_in_creation_order() {
    // Rows differ along the size of the collection: none at all, one, a few, and one past
    // the default page size, each spanning the flag combinations and the absent name.
    let sizes = [0, 1, 2, 5, PAST_DEFAULT_PAGE];

    let gateway = Gateway::new();
    for count in sizes {
        let case = format!("GetRequestValidators over {count} validator(s)");
        let (api, ids) =
            api_with_validators(&gateway, &format!("validator-listing-{count}"), count).await;

        // `limit=500` is the published maximum page size, so one call holds the whole
        // collection (every row here is well under it).
        let (items, next) =
            listed_validators(&gateway, &api, &format!("limit={MAX_PAGE_LIMIT}"), &case).await;
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
        // rendering of the `RequestValidator` shape, not two.
        for (index, item) in items.iter().enumerate() {
            let id = &ids[index];
            let read = get_validator(&gateway, &api, id).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected 200 from GetRequestValidator({id}), got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                item, &read.body,
                "case {case}: listed item #{index} must equal GetRequestValidator({id}), \
                 got {item:?} against {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn limit_and_position_walk_the_collection_exactly_once() {
    // Rows differ along the page size: one item at a time, sizes that do and do not
    // divide the collection, the documented default, one either side of the collection's
    // own size, the published maximum — and sizes OUTSIDE the published window, which are
    // served rather than refused and must still walk the sequence exactly once.
    let size = i32::try_from(PAST_DEFAULT_PAGE).expect("the collection size fits an i32");
    let limits = [
        1,
        2,
        4,
        size - 1,
        size,
        size + 1,
        DEFAULT_PAGE_LIMIT,
        MAX_PAGE_LIMIT,
        MAX_PAGE_LIMIT + 1,
        0,
        -7,
    ];

    let gateway = Gateway::new();
    let (api, expected) =
        api_with_validators(&gateway, "validator-paging", PAST_DEFAULT_PAGE).await;

    for limit in limits {
        let case = format!("GetRequestValidators(limit={limit})");
        let mut seen: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        // The walk is bounded: a page that reports a cursor must have reported at least
        // one item, so the collection's size bounds the number of pages.
        for page in 0..=expected.len() {
            let query = match &position {
                None => format!("limit={limit}"),
                Some(cursor) => format!("limit={limit}&position={cursor}"),
            };
            let (items, next) = listed_validators(&gateway, &api, &query, &case).await;
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
                        "case {case}: the next position must name a stored validator, \
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
async fn a_validator_is_reported_only_through_the_api_that_holds_it() {
    let gateway = Gateway::new();
    let (first, first_ids) = api_with_validators(&gateway, "validator-listing-own", 3).await;
    let (second, second_ids) = api_with_validators(&gateway, "validator-listing-other", 2).await;

    for (case, api, expected) in [
        ("the first API", &first, &first_ids),
        ("the second API", &second, &second_ids),
    ] {
        let (items, _) = listed_validators(&gateway, api, "limit=500", case).await;
        let listed: Vec<String> = items
            .iter()
            .map(|item| string_member(item, "id", case))
            .collect();
        assert_eq!(
            &listed, expected,
            "case {case}: an API reports only its own validators, expected {expected:?}, \
             got {listed:?}"
        );
    }

    // An unknown `restApiId` resolves to no API at all, which is a 404 — not an empty
    // page.
    assert_failure(
        &get_request_validators(&gateway, "zzzzzzzzzz", "").await,
        404,
        "NotFoundException",
        "GetRequestValidators on an unknown API",
    );

    // The validators of a deleted API go with it, and so does its collection.
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{first}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &get_request_validators(&gateway, &first, "").await,
        404,
        "NotFoundException",
        "GetRequestValidators after DeleteRestApi",
    );
}

#[tokio::test]
async fn a_position_naming_nothing_in_this_apis_sequence_is_refused() {
    let gateway = Gateway::new();
    let (api, ids) = api_with_validators(&gateway, "validator-cursor", 3).await;
    let (other, other_ids) = api_with_validators(&gateway, "validator-cursor-other", 1).await;

    // Rows differ along the form of the cursor: one of an id's own form that no validator
    // carries, one that is well formed but names a validator of ANOTHER API, one of a form
    // no id takes, and one far longer than an id.
    for (case, cursor) in [
        ("a cursor no validator carries", "zzzzzzzzzz".to_string()),
        ("a cursor from another API", other_ids[0].clone()),
        ("a cursor of a form no id takes", "NOT-AN-ID".to_string()),
        ("a cursor far longer than an id", "q".repeat(64)),
    ] {
        assert_failure(
            &get_request_validators(&gateway, &api, &format!("position={cursor}")).await,
            400,
            "BadRequestException",
            case,
        );
    }

    // A cursor this API's own sequence carries is served, and resumes the page AT it.
    for (index, id) in ids.iter().enumerate() {
        let case = format!("GetRequestValidators(position={id})");
        let (items, _) =
            listed_validators(&gateway, &api, &format!("position={id}&limit=500"), &case).await;
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
    let (items, _) = listed_validators(&gateway, &other, "limit=500", "the other API").await;
    assert_eq!(
        items.len(),
        other_ids.len(),
        "case the other API: expected {} validator(s), got {items:?}",
        other_ids.len()
    );
}

#[tokio::test]
async fn listing_validators_changes_nothing_a_later_caller_sees() {
    let gateway = Gateway::new();
    let (api, ids) = api_with_validators(&gateway, "validator-listing-stable", 4).await;

    let mut rounds: Vec<Vec<Value>> = Vec::new();
    for round in 0..3 {
        let case = format!("GetRequestValidators(round {round})");
        let (items, _) = listed_validators(&gateway, &api, "limit=500", &case).await;
        rounds.push(items);
    }
    for (round, pair) in rounds.windows(2).enumerate() {
        assert_eq!(
            pair[0], pair[1],
            "case GetRequestValidators round {round} then {}: a report must not change the \
             records, got {:?} then {:?}",
            round + 1,
            pair[0],
            pair[1]
        );
    }

    // And a read of one record after the listings is still the record that was listed.
    for (index, id) in ids.iter().enumerate() {
        let case = format!("GetRequestValidator({id}) after listing");
        let read = get_validator(&gateway, &api, id).await;
        assert_eq!(
            read.body, rounds[0][index],
            "case {case}: expected the listed record {:?}, got {:?}",
            rounds[0][index], read.body
        );
    }

    // A validator created after the listings joins the end of the sequence: the listing
    // did not disturb the creation order the next page is cut out of.
    let appended = created_validator(
        &gateway,
        &api,
        "CreateRequestValidator after listing",
        validator_of_each_shape(ids.len()),
    )
    .await;
    let appended_id = string_member(
        &appended,
        "id",
        "CreateRequestValidator after listing",
    );
    let (items, _) = listed_validators(&gateway, &api, "limit=500", "after the append").await;
    let listed: Vec<String> = items
        .iter()
        .map(|item| string_member(item, "id", "after the append"))
        .collect();
    let mut expected = ids.clone();
    expected.push(appended_id);
    assert_eq!(
        listed, expected,
        "case after the append: expected creation order {expected:?}, got {listed:?}"
    );
}
