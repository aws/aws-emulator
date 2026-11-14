// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetModels`: the paginated report of one API's models.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `CreateModel` / `GetModel` helpers are this crate's existing
//! harness, reused from `rest_api_lifecycle` and `models`.
//!
//! The cases assert RULES over SETS of inputs rather than one recorded call: that the
//! report is exactly the models stored under the named API, in CREATION order (over a
//! table of collection sizes from none created at all to past the default page size, each
//! spanning the content-type families and optional members a model can carry, and named so
//! that creation order is the REVERSE of name order); that an API's listing opens with the
//! models it was CREATED with, which precede anything a caller created; that each listed
//! item is member for member what `GetModel` reports for the same model; that
//! `limit`/`position` walk that sequence exactly once for every page size, including sizes
//! outside the published window; that a model is reported only through the API that holds
//! it; that an unresolvable cursor and an unknown API are refused; and that listing changes
//! nothing a later caller sees.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT,
};

use crate::models::{created_model, default_model_names, get_model};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, string_member, Answer, Gateway,
};

/// `GetModels` for `api` with `query` appended verbatim (empty for no query string).
async fn get_models(gateway: &Gateway, api: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("/restapis/{api}/models")
    } else {
        format!("/restapis/{api}/models?{query}")
    };
    gateway.call("GET", &uri, None).await
}

/// `GetModels` for `api`, asserting the modelled 200 and answering the items of the page
/// beside the cursor it reports for the next one.
async fn listed_models(
    gateway: &Gateway,
    api: &str,
    query: &str,
    case: &str,
) -> (Vec<Value>, Option<String>) {
    let answer = get_models(gateway, api, query).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetModels, got {} with {:?}",
        answer.status, answer.body
    );
    let next = answer.body["position"].as_str().map(str::to_owned);
    (page_items(&answer.body, case), next)
}

/// The name the model created `index`th carries: a letter walking the alphabet BACKWARDS
/// with the index appended, so a collection's creation order is the exact reverse of its
/// name order. A listing that reported stored-map order rather than creation order would
/// therefore answer the sequence reversed, not the sequence asked for.
///
/// "The name of the model. Must be alphanumeric." (`CreateModel$name`), which a letter
/// followed by digits is.
fn model_name(index: usize) -> String {
    let letter = char::from(b'z' - u8::try_from(index % 26).expect("the index fits a byte"));
    format!("{letter}{index}")
}

/// One `CreateModel` request per `index`, rotating over what a model can carry: the
/// content-type family of the payload it describes (`application/json`, a `+json`
/// structured syntax suffix, XML, plain text), whether the request sets `description`,
/// and whether it sets `schema` at all. So a collection of any size holds models of
/// every shape rather than copies of one.
fn model_of_each_shape(index: usize) -> Value {
    let name = model_name(index);
    let content_type = match index % 4 {
        0 => "application/json",
        1 => "application/vnd.pet+json",
        2 => "application/xml",
        _ => "text/plain",
    };
    let mut request = json!({ "name": name, "contentType": content_type });
    if index.is_multiple_of(2) {
        request["description"] = json!(format!("model {index}"));
    }
    // Every third request leaves `schema` out, which a stored model reports as absent;
    // the others carry a document their own content type can describe.
    if !index.is_multiple_of(3) {
        let schema = if content_type.contains("json") {
            json!({ "type": "object", "title": name }).to_string()
        } else {
            format!("<xs:schema><xs:element name=\"{name}\"/></xs:schema>")
        };
        request["schema"] = json!(schema);
    }
    request
}

/// Create `count` models on a fresh API, answering the API id and the names of EVERY model
/// the API then holds, in the order they were created.
///
/// A created API already holds the models API Gateway seeds it with, and those were created
/// with the API, so they lead the sequence a listing reports.
async fn api_with_models(gateway: &Gateway, name: &str, count: usize) -> (String, Vec<String>) {
    let (api, _root) = create_api(gateway, name).await;
    let mut names: Vec<String> = default_model_names();
    for index in 0..count {
        let case = format!("{name}: CreateModel(#{index})");
        let created = created_model(gateway, &api, &case, model_of_each_shape(index)).await;
        names.push(string_member(&created, "name", &case));
    }
    (api, names)
}

/// A collection size past the documented default page size, so a listing that ignored
/// `limit` could not pass the cases below by accident.
const PAST_DEFAULT_PAGE: usize = DEFAULT_PAGE_LIMIT as usize + 1;

#[tokio::test]
async fn get_models_reports_exactly_the_stored_models_in_creation_order() {
    // Rows differ along the size of the collection: none at all, one, a few, and one past
    // the default page size, each spanning the content types, descriptions and schema
    // forms `model_of_each_shape` rotates over.
    let sizes = [0, 1, 2, 5, PAST_DEFAULT_PAGE];

    let gateway = Gateway::new();
    for count in sizes {
        let case = format!("GetModels over {count} model(s)");
        let (api, names) = api_with_models(&gateway, &format!("model-listing-{count}"), count).await;

        // `limit=500` is the published maximum page size, so one call holds the whole
        // collection (every row here is well under it).
        let (items, next) =
            listed_models(&gateway, &api, &format!("limit={MAX_PAGE_LIMIT}"), &case).await;
        assert_eq!(
            next, None,
            "case {case}: a page that holds the whole collection reports no cursor, got \
             {next:?}"
        );
        let listed: Vec<String> = items
            .iter()
            .map(|item| string_member(item, "name", &case))
            .collect();
        assert_eq!(
            listed, names,
            "case {case}: expected the stored models in creation order {names:?}, \
             got {listed:?}"
        );

        // Each listed item is the record a direct read reports, member for member: one
        // rendering of the `Model` shape, not two.
        for (index, item) in items.iter().enumerate() {
            let name = &names[index];
            let read = get_model(&gateway, &api, name, false).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected 200 from GetModel({name}), got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                item, &read.body,
                "case {case}: listed item #{index} must equal GetModel({name}), got {item:?} \
                 against {:?}",
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
    let (api, expected) = api_with_models(&gateway, "model-paging", PAST_DEFAULT_PAGE).await;

    for limit in limits {
        let case = format!("GetModels(limit={limit})");
        let mut seen: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        // The walk is bounded: a page that reports a cursor must have reported at least
        // one item, so the collection's size bounds the number of pages.
        for page in 0..=expected.len() {
            let query = match &position {
                None => format!("limit={limit}"),
                Some(cursor) => format!("limit={limit}&position={cursor}"),
            };
            let (items, next) = listed_models(&gateway, &api, &query, &case).await;
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
            seen.extend(items.iter().map(|item| string_member(item, "name", &case)));
            match next {
                None => break,
                Some(cursor) => {
                    assert!(
                        !cursor.is_empty(),
                        "case {case}: page {page} reported an empty next position"
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
async fn a_model_is_reported_only_through_the_api_that_holds_it() {
    let gateway = Gateway::new();
    let (first, first_names) = api_with_models(&gateway, "model-listing-own", 3).await;
    let (second, second_names) = api_with_models(&gateway, "model-listing-other", 2).await;

    for (case, api, expected) in [
        ("the first API", &first, &first_names),
        ("the second API", &second, &second_names),
    ] {
        let (items, _) = listed_models(&gateway, api, "limit=500", case).await;
        let listed: Vec<String> = items
            .iter()
            .map(|item| string_member(item, "name", case))
            .collect();
        assert_eq!(
            &listed, expected,
            "case {case}: an API reports only its own models, expected {expected:?}, \
             got {listed:?}"
        );
    }

    // Two APIs may each hold a model of ONE name, and each listing reports its own
    // record: the id and the content type the model was created with under that API. The
    // name is one the first API's CALLER created, so the collision is between two created
    // records rather than between a create and a seeded default.
    let shared_name = first_names[default_model_names().len()].clone();
    let (third, _root) = create_api(&gateway, "model-listing-shared-name").await;
    let shared = created_model(
        &gateway,
        &third,
        "the third API's model of a name the first API also holds",
        json!({ "name": shared_name.clone(), "contentType": "text/plain" }),
    )
    .await;
    let (items, _) = listed_models(&gateway, &third, "limit=500", "the third API").await;
    let listed: Vec<String> = items
        .iter()
        .map(|item| string_member(item, "name", "the third API"))
        .collect();
    assert_eq!(
        listed,
        crate::models::with_default_models(std::slice::from_ref(&shared_name)),
        "case the third API: expected its own models only, got {listed:?}"
    );
    assert!(
        items.contains(&shared),
        "case the third API: expected its own record {shared:?} in {items:?}"
    );
    let (first_items, _) = listed_models(
        &gateway,
        &first,
        "limit=500",
        "the first API after the shared name",
    )
    .await;
    let held_here = first_items
        .iter()
        .find(|item| string_member(item, "name", "the first API's model") == shared_name)
        .unwrap_or_else(|| panic!("case the shared name: the first API still holds {shared_name}"));
    assert_ne!(
        string_member(held_here, "id", "the first API's model"),
        string_member(&shared, "id", "the third API's model"),
        "case the shared name: expected each API's model to carry its own id"
    );

    // An unknown `restApiId` resolves to no API at all, which is a 404 — not an empty
    // page.
    assert_failure(
        &get_models(&gateway, "zzzzzzzzzz", "").await,
        404,
        "NotFoundException",
        "GetModels on an unknown API",
    );

    // The models of a deleted API go with it, and so does its collection.
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{first}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &get_models(&gateway, &first, "").await,
        404,
        "NotFoundException",
        "GetModels after DeleteRestApi",
    );
}

#[tokio::test]
async fn a_position_naming_nothing_in_this_apis_sequence_is_refused() {
    let gateway = Gateway::new();
    let (api, names) = api_with_models(&gateway, "model-cursor", 3).await;
    // The other API holds ONE model, under a name this API holds none under, so a cursor
    // naming it is well formed and still resolves to nothing in this API's sequence.
    let (other, _other_root) = create_api(&gateway, "model-cursor-other").await;
    let elsewhere = "Elsewhere";
    created_model(
        &gateway,
        &other,
        "the other API's only model",
        json!({ "name": elsewhere, "contentType": "application/json" }),
    )
    .await;

    // Rows differ along the form of the cursor: one of a model name's own form that no
    // model carries anywhere, one that names a model of ANOTHER API, one of a form no
    // name takes at all, and one far longer than a name.
    for (case, cursor) in [
        ("a cursor no model carries", "Nope".to_string()),
        ("a cursor from another API", elsewhere.to_string()),
        ("a cursor of a form no name takes", "NOT-A-NAME".to_string()),
        ("a cursor far longer than a name", "q".repeat(64)),
    ] {
        assert!(
            !names.contains(&cursor),
            "case {case}: the row must name no model of this API, got {cursor:?}"
        );
        assert_failure(
            &get_models(&gateway, &api, &format!("position={cursor}")).await,
            400,
            "BadRequestException",
            case,
        );
    }

    // Every cursor the service ITSELF handed back is served, and starts the page at the
    // item that follows the ones already reported: the cursors are collected from a
    // one-item-at-a-time walk, so nothing here assumes their form.
    let mut cursors: Vec<String> = Vec::new();
    let mut position: Option<String> = None;
    for _ in 0..names.len() {
        let query = match &position {
            None => "limit=1".to_string(),
            Some(cursor) => format!("limit=1&position={cursor}"),
        };
        let (_, next) = listed_models(&gateway, &api, &query, "collecting the cursors").await;
        match next {
            None => break,
            Some(cursor) => {
                cursors.push(cursor.clone());
                position = Some(cursor);
            }
        }
    }
    assert_eq!(
        cursors.len(),
        names.len().saturating_sub(1),
        "case collecting the cursors: expected one cursor for every element the walk \
         carried on at, got {cursors:?}"
    );
    for (index, cursor) in cursors.iter().enumerate() {
        let case = format!("GetModels(position={cursor})");
        let (items, _) =
            listed_models(&gateway, &api, &format!("position={cursor}&limit=500"), &case).await;
        let listed: Vec<String> = items
            .iter()
            .map(|item| string_member(item, "name", &case))
            .collect();
        // The cursor a one-item page handed back names the element AFTER the one it
        // reported, and a page resumes AT its cursor.
        assert_eq!(
            listed,
            names[index + 1..].to_vec(),
            "case {case}: the page resumes at the cursor, expected {:?}, got {listed:?}",
            &names[index + 1..]
        );
    }

    // The refused cursors took nothing away: the other API still reports its own records —
    // the one its caller created, beside the defaults it was created with.
    let (items, _) = listed_models(&gateway, &other, "limit=500", "the other API").await;
    let listed: Vec<String> = items
        .iter()
        .map(|item| string_member(item, "name", "the other API"))
        .collect();
    assert_eq!(
        listed,
        crate::models::with_default_models(&[elsewhere]),
        "case the other API: expected its own models, got {listed:?}"
    );
}

#[tokio::test]
async fn listing_models_changes_nothing_a_later_caller_sees() {
    let gateway = Gateway::new();
    let (api, names) = api_with_models(&gateway, "model-listing-stable", 4).await;

    let mut rounds: Vec<Vec<Value>> = Vec::new();
    for round in 0..3 {
        let case = format!("GetModels(round {round})");
        let (items, _) = listed_models(&gateway, &api, "limit=500", &case).await;
        rounds.push(items);
    }
    for (round, pair) in rounds.windows(2).enumerate() {
        assert_eq!(
            pair[0], pair[1],
            "case GetModels round {round} then {}: a report must not change the records, \
             got {:?} then {:?}",
            round + 1,
            pair[0],
            pair[1]
        );
    }

    // And a read of one record after the listings is still the record that was listed.
    for (index, name) in names.iter().enumerate() {
        let case = format!("GetModel({name}) after listing");
        let read = get_model(&gateway, &api, name, false).await;
        assert_eq!(
            read.body, rounds[0][index],
            "case {case}: expected the listed record {:?}, got {:?}",
            rounds[0][index], read.body
        );
    }

    // A model created after the listings joins the END of the sequence: the listing did
    // not disturb the creation order the next page is cut out of, and the new model's
    // name sorts BEFORE every earlier one, so name order could not produce this answer.
    let appended = created_model(
        &gateway,
        &api,
        "CreateModel after listing",
        model_of_each_shape(names.len()),
    )
    .await;
    let appended_name = string_member(&appended, "name", "CreateModel after listing");
    let (items, _) = listed_models(&gateway, &api, "limit=500", "after the append").await;
    let listed: Vec<String> = items
        .iter()
        .map(|item| string_member(item, "name", "after the append"))
        .collect();
    let mut expected = names.clone();
    expected.push(appended_name);
    assert_eq!(
        listed, expected,
        "case after the append: expected creation order {expected:?}, got {listed:?}"
    );

    // Deleting a model takes it out of the collection and leaves the order of the rest
    // alone: the sequence is the stored records, not a list built at create time.
    let removed = &expected[1];
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{api}/models/{removed}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteModel({removed}): expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    let (items, _) = listed_models(&gateway, &api, "limit=500", "after the delete").await;
    let listed: Vec<String> = items
        .iter()
        .map(|item| string_member(item, "name", "after the delete"))
        .collect();
    let survivors: Vec<String> = expected
        .iter()
        .filter(|name| *name != removed)
        .cloned()
        .collect();
    assert_eq!(
        listed, survivors,
        "case after the delete: expected {survivors:?}, got {listed:?}"
    );
}
