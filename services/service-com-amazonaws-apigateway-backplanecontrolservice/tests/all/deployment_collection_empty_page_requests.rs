// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the answer the DEPLOYMENTS collection of one API makes when its
//! page holds no element, read across every request form the listing accepts and across
//! the states a store reaches it from.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`Gateway`], `create_api`, `page_items` and
//! `string_member` are this crate's one wire-test harness, reused from
//! `rest_api_lifecycle`; a deployment is created through `stage_lifecycle`, and the empty
//! page's whole body is this crate's one reading of it
//! (`crate::emptied_collection_page_member::empty_page_body`).
//!
//! The rule, asserted over a TABLE rather than one call: a `position`/`limit` listing
//! reports "the current page of elements from this collection" as a PRESENT member — the
//! empty list where the page holds no element — for every request the listing accepts and
//! whatever else the store holds, so a client reads "nothing is listed" off a list it can
//! count rather than off an absent member.
//!
//! `crate::deployment_collection` owns the collection's contents and asserts its empty
//! answer for a plain read, a read after a REFUSED write and a read after a DENIED one;
//! `crate::collection_empty_page_member` reads every listing of this service once and
//! crosses the published page sizes with the collections that publish a filter, which the
//! deployments listing does not. What this suite adds is the deployments listing's empty
//! answer under the rest of its own request surface — every page size the `limit` member
//! publishes with one value past each bound — crossed with the ways this API's page comes
//! to hold nothing while a store around it holds deployments of other APIs.
//!
//! Source of the page-size range: "The maximum number of returned results per page. The
//! default value is 25 and the maximum value is 500"
//! (`smithy_query(query="doc", shape="GetDeploymentsRequest", member="limit",
//! model_path="services/service-com-amazonaws-apigateway-backplanecontrolservice/\
//! smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json")`), and the page
//! member itself is `items` (`jsonName` `item`) of the `Deployments` output shape.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT,
};

use crate::deployment_collection::get_deployments;
use crate::rest_api_lifecycle::{create_api, page_items, string_member, Gateway};
use crate::stage_lifecycle::create_deployment;

/// The page sizes a request may ask the listing for: one below the published minimum, the
/// minimum itself, the documented default, the published maximum, one past it, and far past
/// it — plus the absent size, which the empty query at the head of the list stands for.
fn page_size_queries() -> Vec<String> {
    let mut queries = vec![String::new()];
    for requested in [
        -1,
        0,
        1,
        i64::from(DEFAULT_PAGE_LIMIT),
        i64::from(MAX_PAGE_LIMIT),
        i64::from(MAX_PAGE_LIMIT) + 1,
        100_000,
    ] {
        queries.push(format!("?limit={requested}"));
    }
    queries
}

/// Delete one deployment of `api`, asserting the 202 a delete of this service answers.
async fn delete_deployment(gateway: &Gateway, api: &str, deployment: &str, case: &str) {
    let answer = gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/deployments/{deployment}"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 202,
        "case {case}: expected 202 from DeleteDeployment, got {} with {:?}",
        answer.status, answer.body
    );
}

/// Read the deployments of `api` under every page size the `limit` member publishes, and
/// assert each answer is the whole body an empty page is reported with — twice per query,
/// because a listing is a reporting operation and must leave the next caller the same
/// answer.
async fn assert_empty_under_every_page_size(gateway: &Gateway, api: &str, state: &str) {
    let expected = crate::emptied_collection_page_member::empty_page_body();
    for query in page_size_queries() {
        let case = format!("GetDeployments({state}, {query:?})");
        for round in 1..=2 {
            let answer = get_deployments(gateway, api, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case} (round {round}): expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body, expected,
                "case {case} (round {round}): expected {expected:?}, got {:?}",
                answer.body
            );
        }
    }
}

/// Read the deployments of `api` under every page size, and assert each page reports the
/// `expected` ids in the order the answer lists them, held inside the size it was served.
///
/// The positive control beside [`assert_empty_under_every_page_size`]: a page is empty
/// because the collection holds nothing, not because the listing answers an empty page
/// whatever it holds.
async fn assert_reports_under_every_page_size(
    gateway: &Gateway,
    api: &str,
    expected: &[String],
    state: &str,
) {
    for size in page_size_queries() {
        let case = format!("GetDeployments({state}, {size:?})");
        let mut seen: Vec<String> = Vec::new();
        let mut query = size.clone();
        loop {
            let answer = get_deployments(gateway, api, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let items = page_items(&answer.body, &case);
            seen.extend(items.iter().map(|item| string_member(item, "id", &case)));
            let Some(cursor) = answer.body["position"].as_str() else {
                break;
            };
            // The page the collection carries on at, asked for under the same size.
            query = if size.is_empty() {
                format!("?position={cursor}")
            } else {
                format!("{size}&position={cursor}")
            };
        }
        assert_eq!(
            seen, expected,
            "case {case}: expected the collection {expected:?} in creation order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn the_deployments_of_an_api_that_holds_none_report_the_empty_page_under_every_request() {
    // The ways this API's page comes to hold nothing while the listing still resolves,
    // read on a store that holds deployments of OTHER APIs — so what the answer reports is
    // this API's collection and not whatever the service happens to hold: the API never
    // held a deployment, the API was emptied of its only one, and the API was emptied of
    // several.
    let gateway = Gateway::new();

    // The sibling APIs whose deployments the store holds throughout, one with a single
    // deployment and one with three.
    let (one_deployment, _root) = create_api(&gateway, "empty-page-sibling-one").await;
    create_deployment(
        &gateway,
        &one_deployment,
        "CreateDeployment(sibling with one)",
        json!({}),
    )
    .await;
    let (three_deployments, _root) = create_api(&gateway, "empty-page-sibling-three").await;
    let mut siblings: Vec<String> = Vec::new();
    for index in 0..3 {
        siblings.push(
            create_deployment(
                &gateway,
                &three_deployments,
                &format!("CreateDeployment(sibling #{index})"),
                json!({ "description": format!("sibling deployment {index}") }),
            )
            .await,
        );
    }

    // Row 1: an API that never held a deployment — the read an "is it deployed?" probe
    // makes before anything was created.
    let (never, _root) = create_api(&gateway, "empty-page-never-deployed").await;
    assert_empty_under_every_page_size(&gateway, &never, "an api that never held one").await;

    // Row 2: an API emptied of its only deployment.
    let (emptied_one, _root) = create_api(&gateway, "empty-page-emptied-one").await;
    let only = create_deployment(
        &gateway,
        &emptied_one,
        "CreateDeployment(the only one)",
        json!({ "description": "the only deployment" }),
    )
    .await;
    assert_reports_under_every_page_size(
        &gateway,
        &emptied_one,
        std::slice::from_ref(&only),
        "an api holding one",
    )
    .await;
    delete_deployment(&gateway, &emptied_one, &only, "the only deployment").await;
    assert_empty_under_every_page_size(&gateway, &emptied_one, "an api emptied of one").await;

    // Row 3: an API emptied of several deployments, one delete at a time — the page shrinks
    // through every count down to none, and the member is present at each of them.
    let (emptied_many, _root) = create_api(&gateway, "empty-page-emptied-many").await;
    let mut held: Vec<String> = Vec::new();
    for index in 0..3 {
        held.push(
            create_deployment(
                &gateway,
                &emptied_many,
                &format!("CreateDeployment(#{index})"),
                json!({ "description": format!("deployment {index}") }),
            )
            .await,
        );
    }
    while let Some(oldest) = held.first().cloned() {
        assert_reports_under_every_page_size(
            &gateway,
            &emptied_many,
            &held,
            &format!("an api holding {}", held.len()),
        )
        .await;
        delete_deployment(&gateway, &emptied_many, &oldest, "one of several").await;
        held.remove(0);
    }
    assert_empty_under_every_page_size(&gateway, &emptied_many, "an api emptied of several").await;

    // The siblings are untouched by every read and delete above: the empty answers were
    // about the APIs that hold nothing, not about a store that lost its records.
    assert_reports_under_every_page_size(
        &gateway,
        &three_deployments,
        &siblings,
        "the sibling holding three",
    )
    .await;

    // And an API that reported the empty page reports its own deployment once it has one,
    // so the empty list was the collection's state rather than the listing's answer.
    let deployed = create_deployment(
        &gateway,
        &never,
        "CreateDeployment(after the empty reads)",
        json!({}),
    )
    .await;
    assert_reports_under_every_page_size(
        &gateway,
        &never,
        std::slice::from_ref(&deployed),
        "an api deployed after the empty reads",
    )
    .await;
}

#[tokio::test]
async fn an_empty_deployments_page_is_the_same_answer_as_every_other_empty_listing() {
    // The body an empty page is reported with does not change shape with WHICH collection
    // answers it, so the deployments listing of an API that holds none is read beside the
    // other collections that API holds none of.
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "empty-page-shape").await;
    let expected: Value = crate::emptied_collection_page_member::empty_page_body();

    let rows: Vec<(&str, String)> = vec![
        ("GetDeployments", format!("/restapis/{api}/deployments")),
        ("GetAuthorizers", format!("/restapis/{api}/authorizers")),
        (
            "GetDocumentationVersions",
            format!("/restapis/{api}/documentation/versions"),
        ),
        (
            "GetRequestValidators",
            format!("/restapis/{api}/requestvalidators"),
        ),
    ];

    for (case, uri) in &rows {
        let answer = gateway.call("GET", uri, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body, expected,
            "case {case}: expected {expected:?} for a collection holding nothing, got {:?}",
            answer.body
        );
        // The member a client indexes is the one the output shape publishes, and it is a
        // list it can count — a page of none included.
        assert_eq!(
            page_items(&answer.body, case),
            Vec::<Value>::new(),
            "case {case}: expected a countable page of none, got {:?}",
            answer.body
        );
    }
}
