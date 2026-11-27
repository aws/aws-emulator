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
//! being the record the single-deployment read reports; it changes nothing. The page member
//! is reported whatever the page holds, so a listing whose page came out empty is a success
//! carrying an EMPTY `items` list — including after a write this collection REFUSED, which
//! is the read a caller makes to prove the refused call created nothing. So the tests
//! assert over SETS of inputs — a table of deployment shapes and counts (including none
//! at all), the whole documented page-size range with one value past each bound, a cursor
//! that names nothing, APIs that own the collection and APIs that do not, and a table of
//! the ways a `CreateDeployment` on the collection is refused.

use serde_json::{json, Value};

// The signing helper, the seeded session and the document builder are the siblings' own —
// one spelling of a control-plane request's credential scope and one seeded principal — so
// the suite that denies a write and this one cannot drive that gate with two different ones.
use service_com_amazonaws_apigateway_backplanecontrolservice::state::MAX_STRING_CHARS;

use crate::control_plane_fidelity_rules::call_as;
use crate::execute_api_data_plane::put_integration;
use crate::rest_api_lifecycle::{
    assert_failure, create_api, create_resource, page_items, string_member, Answer, Gateway,
};
use crate::stage_invoke_identity_policy::{
    allow, given_policy, given_session, Entity, SESSION_KEY_ID,
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
    // The method's integration, which is what a stage serves a matched request with: a
    // deployment of a method that has none is refused, so the API is only deployable once
    // its method carries one. `MOCK` is the type that needs no backend at all.
    put_integration(gateway, &api, &resource_id, "GET", json!({ "type": "MOCK" })).await;
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
            "a long description and a canary on a stage already deployed",
            json!({
                "description": "d".repeat(512),
                // A canary is a share of an EXISTING stage's traffic, so it names the
                // stage an earlier row deployed rather than a new one.
                "stageName": "prod",
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

    // A cursor names the first deployment the previous page did NOT hand out, so a page
    // requested at one lists the collection FROM that deployment on. The cursors are read
    // off a one-at-a-time walk, because a `position` is a continuation token this service
    // mints for the walk rather than a deployment id a caller reads off the page.
    let mut index = 0usize;
    let mut cursor: Option<String> = None;
    loop {
        let case = format!("GetDeployments(limit=1) page {index}");
        let query = match &cursor {
            None => "?limit=1".to_string(),
            Some(position) => format!("?limit=1&position={position}"),
        };
        let answer = get_deployments(&gateway, &api, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        index += page_items(&answer.body, &case).len();
        let Some(next) = answer.body["position"].as_str().map(str::to_owned) else {
            break;
        };
        assert!(
            created.iter().all(|id| !next.contains(id.as_str())),
            "case {case}: a cursor spells no deployment's id, got {next:?}"
        );
        let case = format!("position=<the cursor of index {index}>");
        let answer = get_deployments(&gateway, &api, &format!("?position={next}")).await;
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
        cursor = Some(next);
    }
    assert_eq!(
        index,
        created.len(),
        "case GetDeployments(limit=1): the walk must reach every deployment of {created:?}"
    );

    // A value this service did not issue as a cursor names no position, so it is a bad
    // request rather than an empty page — a deployment's own id among them, since the
    // cursor is the service's token and not an identifier a caller holds.
    let own_id = created[1].clone();
    for cursor in ["nosuchcursor", "0123456789", "ZZZZZZ", own_id.as_str()] {
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
async fn the_page_member_is_reported_where_the_page_holds_an_element() {
    let gateway = Gateway::new();
    let api = api_with_a_method(&gateway, "deployment-page-member").await;

    // An api that never held a deployment: the page holds no element, so the member is
    // present and empty.
    let empty = get_deployments(&gateway, &api, "").await;
    assert_eq!(
        empty.body.get("item"),
        Some(&json!([])),
        "case \"an api that never held a deployment\": expected the empty page member, got {:?}",
        empty.body
    );

    // The populated pages beside it: the member reports one element, then two, in creation
    // order.
    let first =
        create_deployment(&gateway, &api, "CreateDeployment(page member 1)", json!({})).await;
    let one = get_deployments(&gateway, &api, "").await;
    assert_eq!(
        reported_ids(&one.body, "one deployment"),
        vec![first.clone()],
        "case \"one deployment\": expected the page member to carry it, got {:?}",
        one.body
    );
    let second =
        create_deployment(&gateway, &api, "CreateDeployment(page member 2)", json!({})).await;
    let both = get_deployments(&gateway, &api, "").await;
    assert_eq!(
        reported_ids(&both.body, "two deployments"),
        vec![first.clone(), second.clone()],
        "case \"two deployments\": expected both in creation order, got {:?}",
        both.body
    );

    // An api a caller EMPTIED again: the page came out empty a second way, and it is the
    // same answer.
    for deployment in [&first, &second] {
        let deleted = gateway
            .call(
                "DELETE",
                &format!("/restapis/{api}/deployments/{deployment}"),
                None,
            )
            .await;
        assert_eq!(
            deleted.status, 202,
            "case \"an api a caller emptied\": expected 202 from DeleteDeployment, got {} with \
             {:?}",
            deleted.status, deleted.body
        );
    }
    let emptied = get_deployments(&gateway, &api, "").await;
    assert_eq!(
        emptied.body.get("item"),
        Some(&json!([])),
        "case \"an api a caller emptied\": expected the empty page member, got {:?}",
        emptied.body
    );
}

/// One `CreateDeployment` this collection REFUSES, and the modelled failure it answers.
struct Refusal {
    /// The case name every assertion of the row carries.
    case: &'static str,
    /// Whether the row's API carries a method, i.e. has anything to deploy at all.
    method: bool,
    /// The request body the refused call carries.
    request: Value,
    /// The modelled status and `x-amzn-errortype` the refusal answers.
    status: u16,
    code: &'static str,
}

/// A `stageName` carrying a character outside the set the member is spelled from: "Stage
/// names can only contain alphanumeric characters, hyphens, and underscores"
/// (`smithy_query(query="doc",shape="com.amazonaws.apigateway#CreateDeploymentRequest",member="stageName")`).
const STAGE_NAME_WITH_A_SPACE: &str = "not a stage name";

/// The largest share of a stage's traffic a canary may take is 100: "The percent (0-100) of
/// traffic diverted to a canary deployment"
/// (`smithy_query(query="doc",shape="com.amazonaws.apigateway#CanarySettings",member="percentTraffic")`),
/// so this share is one past that bound.
const CANARY_SHARE_PAST_THE_BOUND: f64 = 150.0;

#[tokio::test]
async fn a_refused_deployment_leaves_the_listing_reporting_the_empty_page_member() {
    // The read a caller writes to prove a REFUSED write created nothing is the collection's
    // own listing, so the refusal must leave a listing a client can count — the page member
    // present and empty — rather than one it has to branch on. Rows differ along WHY the
    // write was refused: nothing to deploy at all, a member outside the grammar it
    // publishes, a member past its length bound, and a nested member outside its published
    // range.
    let refusals = vec![
        Refusal {
            case: "an api with no method to deploy",
            method: false,
            request: json!({}),
            status: 400,
            code: "BadRequestException",
        },
        Refusal {
            case: "a stageName outside the character set the member publishes",
            method: true,
            request: json!({ "stageName": STAGE_NAME_WITH_A_SPACE }),
            status: 400,
            code: "BadRequestException",
        },
        Refusal {
            case: "a description past the length a stored string is bounded at",
            method: true,
            // One character past the bound this service holds a caller-supplied string to,
            // read off the constant rather than restated here.
            request: json!({ "description": "d".repeat(MAX_STRING_CHARS + 1) }),
            status: 400,
            code: "BadRequestException",
        },
        Refusal {
            case: "a canary share past the window the member publishes",
            method: true,
            request: json!({
                "stageName": "canary",
                "canarySettings": { "percentTraffic": CANARY_SHARE_PAST_THE_BOUND },
            }),
            status: 400,
            code: "BadRequestException",
        },
    ];

    for refusal in &refusals {
        // A fresh emulator per row, so the collection the listing reads is exactly the one
        // the refused write addressed.
        let gateway = Gateway::new();
        let api = if refusal.method {
            api_with_a_method(&gateway, "deployment-refused-write").await
        } else {
            let (api, _root) = create_api(&gateway, "deployment-refused-write").await;
            api
        };

        let refused = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/deployments"),
                Some(refusal.request.clone()),
            )
            .await;
        assert_failure(&refused, refusal.status, refusal.code, refusal.case);

        let listed = get_deployments(&gateway, &api, "").await;
        assert_eq!(
            listed.status, 200,
            "case {}: expected the collection of the api the refused write addressed to be \
             served, got {} with {:?}",
            refusal.case, listed.status, listed.body
        );
        assert_eq!(
            listed.body.get("item"),
            Some(&json!([])),
            "case {}: expected the empty page member after the refusal, got {:?}",
            refusal.case,
            listed.body
        );
        assert!(
            listed.body["position"].is_null(),
            "case {}: a page the collection holds nothing after reports no cursor, got {:?}",
            refusal.case,
            listed.body["position"]
        );

        // The positive control: the collection is empty because nothing is in it, not
        // because the listing answers an empty page whatever the collection holds.
        let deployed = create_deployment(
            &gateway,
            &api,
            &format!("CreateDeployment after {}", refusal.case),
            json!({}),
        )
        .await;
        assert_eq!(
            reported_ids(
                &get_deployments(&gateway, &api, "").await.body,
                refusal.case
            ),
            vec![deployed],
            "case {}: expected the servable deployment to be on the page",
            refusal.case
        );
    }
}

#[tokio::test]
async fn a_deployment_the_callers_policies_deny_leaves_the_listing_reporting_the_empty_page_member()
{
    // The same effect-absent read for the refusal that is taken AHEAD of the operation: a
    // principal whose identity-based policies allow the plane's reads and no write at all.
    let gateway = Gateway::new();
    given_session(&gateway);
    given_policy(&gateway, Entity::Role, Some(&allow("apigateway:GET", "*")));
    let api = api_with_a_method(&gateway, "deployment-denied-write").await;

    let denied = call_as(
        &gateway,
        SESSION_KEY_ID,
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({})),
    )
    .await;
    assert_failure(
        &denied,
        403,
        "AccessDeniedException",
        "CreateDeployment under a principal with no write grant",
    );

    // Read back both as the account's own credential and as the denied principal itself —
    // whose grant covers the listing — because the read a caller writes after a denial is
    // made with the credential it was denied under.
    for (case, answer) in [
        (
            "the account's own credential",
            get_deployments(&gateway, &api, "").await,
        ),
        (
            "the principal the write was denied for",
            call_as(
                &gateway,
                SESSION_KEY_ID,
                "GET",
                &format!("/restapis/{api}/deployments"),
                None,
            )
            .await,
        ),
    ] {
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the listing to be served, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body.get("item"),
            Some(&json!([])),
            "case {case}: expected the empty page member after the denial, got {:?}",
            answer.body
        );
    }

    // And the denial changed nothing about what a deployment the plane DOES serve reports.
    let deployed = create_deployment(
        &gateway,
        &api,
        "CreateDeployment after the denial",
        json!({}),
    )
    .await;
    assert_eq!(
        reported_ids(
            &get_deployments(&gateway, &api, "").await.body,
            "after the denial"
        ),
        vec![deployed],
        "case after the denial: expected the served deployment to be on the page"
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

    // The cursor the first page of one hands back, which is the only `position` a caller
    // can send: this service mints it rather than a caller spelling it.
    let first_page = get_deployments(&gateway, &api, "?limit=1").await;
    let cursor = first_page.body["position"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case GetDeployments(limit=1): expected a cursor while a deployment remains, \
                 got {:?}",
                first_page.body
            )
        })
        .to_string();

    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        // Rows: the whole collection, one page of it, and the page after that cursor —
        // no shape of read may move the collection on.
        for query in ["", "?limit=1", &format!("?position={cursor}")] {
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
