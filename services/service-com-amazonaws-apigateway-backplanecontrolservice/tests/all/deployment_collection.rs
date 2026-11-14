// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetDeployments`: the collection of an API's deployments.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateDeployment` helpers are this crate's existing harness, reused
//! from `rest_api_lifecycle` and `stage_lifecycle`.
//!
//! The rule under test: the collection reports every stored deployment of the named API
//! in creation order, oldest first, one `position`/`limit` page at a time, each entry
//! being the record the single-deployment read reports; it changes nothing. So the tests
//! assert over SETS of inputs — a table of deployment shapes and counts (including none
//! at all), the whole documented page-size range with one value past each bound, a cursor
//! that names nothing, and APIs that own the collection and APIs that do not.

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{
    assert_failure, create_api, create_resource, page_items, string_member, Answer, Gateway,
};
use crate::stage_lifecycle::create_deployment;

/// `GetDeployments` for `api`, with `query` appended verbatim (`""` for no paging).
/// Shared with the `delete_deployment` suite, which reads the collection back to show
/// which records a delete left alone.
pub(crate) async fn get_deployments(gateway: &Gateway, api: &str, query: &str) -> Answer {
    gateway
        .call("GET", &format!("/restapis/{api}/deployments{query}"), None)
        .await
}

/// `GetDeployment` for one deployment of `api`. Shared with the `delete_deployment`
/// suite, which needs the raw answer (a deleted deployment is read back as a modelled
/// failure, not a 200).
pub(crate) async fn get_deployment(gateway: &Gateway, api: &str, deployment: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/deployments/{deployment}"),
            None,
        )
        .await
}

/// The deployment ids of a `GetDeployments` answer, in the order the answer lists them —
/// the order is itself part of the rule, so it is NOT sorted away here. Shared with the
/// `delete_deployment` suite.
pub(crate) fn reported_ids(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| string_member(item, "id", case))
        .collect()
}

/// An API with one resource carrying one method, so each deployment of it snapshots
/// something.
async fn api_with_a_method(gateway: &Gateway, name: &str) -> String {
    let (api, root) = create_api(gateway, name).await;
    let resource = create_resource(gateway, &api, &root, "pets").await;
    let resource_id = string_member(&resource, "id", name);
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource_id}/methods/GET"),
            Some(json!({ "authorizationType": "NONE" })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case {name}: expected 201 from PutMethod, got {} with {:?}",
        answer.status, answer.body
    );
    api
}

#[tokio::test]
async fn get_deployments_reports_every_deployment_as_get_deployment_reports_it() {
    // Rows differ along what the rule generalises over: which optional members the
    // deployment request carries, how long its description is, whether it also creates a
    // stage, and how many stage variables that stage gets. The collection is read after
    // each row, so the assertion spans a collection of one deployment up to one of five.
    let cases: Vec<(&str, Value)> = vec![
        ("every optional member absent", json!({})),
        (
            "described",
            json!({ "description": "the first deployment" }),
        ),
        (
            "described and deployed to a stage",
            json!({ "description": "to prod", "stageName": "prod" }),
        ),
        (
            "a stage with variables and a cache cluster",
            json!({
                "stageName": "staging",
                "stageDescription": "the staging stage",
                "variables": { "table": "pets", "tier": "gold" },
                "cacheClusterEnabled": true,
                "cacheClusterSize": "0.5"
            }),
        ),
        (
            "a long description and a canary",
            json!({
                "description": "d".repeat(512),
                "stageName": "canary_stage",
                "canarySettings": { "percentTraffic": 10.0 }
            }),
        ),
    ];

    let gateway = Gateway::new();
    let api = api_with_a_method(&gateway, "deployment-collection").await;

    // An API with no deployments has the collection, and it is empty — not a refusal and
    // not an absent member.
    let empty = get_deployments(&gateway, &api, "").await;
    assert_eq!(
        empty.status, 200,
        "case GetDeployments(no deployments): expected 200, got {} with {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        reported_ids(&empty.body, "GetDeployments(no deployments)"),
        Vec::<String>::new(),
        "case GetDeployments(no deployments): expected an empty collection, got {:?}",
        empty.body
    );

    let mut expected: Vec<String> = Vec::new();
    for (case, request) in &cases {
        let created = create_deployment(&gateway, &api, case, request.clone()).await;
        expected.push(created.clone());

        let listed = get_deployments(&gateway, &api, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected 200 from GetDeployments, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            reported_ids(&listed.body, case),
            expected,
            "case {case}: expected every deployment in creation order {expected:?}, got {:?}",
            listed.body
        );

        // Each entry is the stored record: the very body the single-deployment read
        // answers for it.
        let entry = page_items(&listed.body, case)
            .into_iter()
            .find(|item| item["id"].as_str() == Some(created.as_str()))
            .unwrap_or_else(|| panic!("case {case}: the created deployment must be listed"));
        let read = get_deployment(&gateway, &api, &created).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetDeployment, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            entry, read.body,
            "case {case}: a listed deployment must be the record GetDeployment reports, got {entry:?} against {:?}",
            read.body
        );
        // The description the request asked for, reported or absent exactly as asked.
        assert_eq!(
            entry["description"],
            request
                .get("description")
                .cloned()
                .unwrap_or(Value::Null),
            "case {case}: expected the created description, got {:?}",
            entry["description"]
        );
        // `GetDeployments` models no `embed` member, so no listed entry carries an
        // `apiSummary` — even on an API that has a method to summarise.
        assert!(
            entry["apiSummary"].is_null(),
            "case {case}: a listed deployment reports no apiSummary, got {:?}",
            entry["apiSummary"]
        );
    }
}

#[tokio::test]
async fn get_deployments_pages_through_creation_order_on_position_and_limit() {
    let gateway = Gateway::new();
    let api = api_with_a_method(&gateway, "deployment-paging").await;
    let mut created: Vec<String> = Vec::new();
    for n in 0..7 {
        created.push(
            create_deployment(
                &gateway,
                &api,
                &format!("CreateDeployment({n})"),
                json!({ "description": format!("deployment {n}") }),
            )
            .await,
        );
    }

    // With no `limit` the default page size (25) covers all seven, so the one page is the
    // whole collection and the collection carries on nowhere: no cursor.
    let all = get_deployments(&gateway, &api, "").await;
    assert_eq!(
        reported_ids(&all.body, "GetDeployments(no limit)"),
        created,
        "case GetDeployments(no limit): expected creation order {created:?}, got {:?}",
        all.body
    );
    assert_eq!(
        all.body["position"].as_str(),
        None,
        "case GetDeployments(no limit): a page that holds the whole collection reports no \
         cursor, got {:?}",
        all.body["position"]
    );

    // Rows span the documented page-size range — "The default value is 25 and the maximum
    // value is 500" (`GetDeployments`'s `limit` member) — its bounds, and one value past
    // each bound, which is clamped into the range rather than refused.
    for limit in [0, 1, 2, 3, 7, 8, 25, 500, 501, 10_000] {
        let case = format!("limit={limit}");
        let effective = limit.clamp(1, 500) as usize;
        let mut seen: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &position {
                None => format!("?limit={limit}"),
                Some(cursor) => format!("?limit={limit}&position={cursor}"),
            };
            let answer = get_deployments(&gateway, &api, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let items = page_items(&answer.body, &case);
            assert!(
                items.len() <= effective,
                "case {case}: a page holds at most {effective} items, got {}",
                items.len()
            );
            seen.extend(items.iter().map(|item| string_member(item, "id", &case)));
            pages += 1;
            assert!(pages <= 20, "case {case}: pagination did not terminate");
            match answer.body["position"].as_str() {
                None => break,
                Some(cursor) => position = Some(cursor.to_owned()),
            }
        }
        assert_eq!(
            seen, created,
            "case {case}: paging must reassemble creation order {created:?}, got {seen:?}"
        );
    }

    // A cursor is the id of the first deployment the previous page did NOT hand out, so a
    // page requested at one lists the collection FROM that deployment on.
    for (index, cursor) in created.iter().enumerate() {
        let case = format!("position={cursor}");
        let answer = get_deployments(&gateway, &api, &format!("?position={cursor}")).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            reported_ids(&answer.body, &case),
            created[index..].to_vec(),
            "case {case}: expected the collection from the cursor on, got {:?}",
            answer.body
        );
    }

    // A cursor naming nothing in the sequence is a bad request, not an empty page.
    for cursor in ["nosuchcursor", "0123456789", "ZZZZZZ"] {
        assert_failure(
            &get_deployments(&gateway, &api, &format!("?position={cursor}")).await,
            400,
            "BadRequestException",
            &format!("GetDeployments(position={cursor:?})"),
        );
    }
}

#[tokio::test]
async fn the_collection_belongs_to_the_api_that_owns_it() {
    let gateway = Gateway::new();
    let first_api = api_with_a_method(&gateway, "deployment-owner-one").await;
    let second_api = api_with_a_method(&gateway, "deployment-owner-two").await;
    let mine = create_deployment(&gateway, &first_api, "CreateDeployment(owner)", json!({})).await;

    // Rows: the API that has the deployment, a sibling API that has none, and an
    // identifier no API answers to.
    assert_eq!(
        reported_ids(
            &get_deployments(&gateway, &first_api, "").await.body,
            "the owning API"
        ),
        vec![mine.clone()],
        "case the owning API: expected its one deployment"
    );
    assert_eq!(
        reported_ids(
            &get_deployments(&gateway, &second_api, "").await.body,
            "a sibling API"
        ),
        Vec::<String>::new(),
        "case a sibling API: another API's deployment must not be listed on it"
    );
    assert_failure(
        &get_deployments(&gateway, "zzzzzzzzzz", "").await,
        404,
        "NotFoundException",
        "GetDeployments on an unknown restApiId",
    );
    // And the sibling's own deployment joins only the sibling's collection.
    let theirs =
        create_deployment(&gateway, &second_api, "CreateDeployment(sibling)", json!({})).await;
    assert_eq!(
        reported_ids(
            &get_deployments(&gateway, &second_api, "").await.body,
            "the sibling after its own deployment"
        ),
        vec![theirs],
        "case the sibling after its own deployment: expected only its own record"
    );
    assert_eq!(
        reported_ids(
            &get_deployments(&gateway, &first_api, "").await.body,
            "the first API after the sibling deployed"
        ),
        vec![mine],
        "case the first API after the sibling deployed: expected only its own record"
    );

    // Deleting the API takes the whole collection with it.
    let dropped = gateway
        .call("DELETE", &format!("/restapis/{first_api}"), None)
        .await;
    assert_eq!(
        dropped.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        dropped.status, dropped.body
    );
    assert_failure(
        &get_deployments(&gateway, &first_api, "").await,
        404,
        "NotFoundException",
        "GetDeployments after DeleteRestApi",
    );
}

#[tokio::test]
async fn listing_the_deployments_does_not_change_what_a_later_caller_sees() {
    let gateway = Gateway::new();
    let api = api_with_a_method(&gateway, "deployment-collection-stable").await;
    let first = create_deployment(&gateway, &api, "CreateDeployment(stable 1)", json!({})).await;
    let second = create_deployment(
        &gateway,
        &api,
        "CreateDeployment(stable 2)",
        json!({ "description": "the second" }),
    )
    .await;

    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        // Rows: the whole collection, one page of it, and the page after that cursor —
        // no shape of read may move the collection on.
        for query in ["", "?limit=1", &format!("?position={second}")] {
            let answer = get_deployments(&gateway, &api, query).await;
            assert_eq!(
                answer.status, 200,
                "case GetDeployments({query}, round {round}): expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            if query.is_empty() {
                seen.push(answer.body);
            }
        }
    }
    assert_eq!(
        seen[0], seen[2],
        "case GetDeployments repeated: a read must not change the collection, got {:?} then {:?}",
        seen[0], seen[2]
    );
    assert_eq!(
        reported_ids(&seen[2], "GetDeployments after the listings"),
        vec![first.clone(), second.clone()],
        "case GetDeployments after the listings: expected both deployments in creation order"
    );
    // And it changed nothing a single-deployment read sees either.
    let read = get_deployment(&gateway, &api, &first).await;
    assert_eq!(
        read.status, 200,
        "case GetDeployment after the listings: expected 200, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.body["id"].as_str(),
        Some(first.as_str()),
        "case GetDeployment after the listings: expected the stored record, got {:?}",
        read.body
    );
}
