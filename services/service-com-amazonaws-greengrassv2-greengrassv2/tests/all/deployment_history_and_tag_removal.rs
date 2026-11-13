// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Greengrass V2 deployment HISTORY and for removing a
//! resource's tags — the three routes this suite drives are `ListDeployments`,
//! `DeleteDeployment` and `UntagResource`.
//!
//! The rules under test:
//!
//! * `ListDeployments` answers from the deployments this account recorded, newest first:
//!   the latest revision of each target by default ("Default: LATEST_ONLY") and every
//!   revision under `historyFilter=ALL`, narrowed to the deployments of one `targetArn`
//!   or of one `parentTargetArn` when the request names one, a page at a time under
//!   `maxResults`, with every entry's members the ones that deployment's own reader
//!   reports;
//! * `DeleteDeployment` deletes the deployment its `deploymentId` names, and refuses one
//!   that is still ACTIVE — "to delete an active deployment, you must first cancel it" —
//!   while an id this account holds no deployment under is the deployment surface's
//!   modelled 404 with one fixed sentence; a deleted deployment is gone from the reader
//!   and from the listing, and the revision that survives it is the latest for its
//!   target;
//! * `UntagResource` removes exactly the keys it names from the tag set the resource
//!   itself holds, leaves the rest, is idempotent in a key the resource does not carry,
//!   and refuses an ARN that names no resource this service holds with the tagging
//!   route's OWN not-found class;
//! * and an operation whose modelled output structure declares no member answers
//!   `204 No Content` with no body at all (`DeleteDeployment`, `UntagResource`,
//!   `TagResource`).
//!
//! Every assertion drives the real `restJson1` front door: the request is built the way
//! an SDK builds it, dispatched through [`MultiServiceRouter`], and judged on the HTTP
//! status and the JSON body. Nothing reaches into the state struct, so a listing served
//! from anything but what the creates persisted fails here.
//!
//! The rows differ along what the rules generalise over — the target FORM (a thing, a
//! thing group, a subdeployment under a parent), how many revisions a target has, the
//! history filter, the page size (at both published bounds and past the upper one), which
//! status a deployment is in when it is deleted, and how many of a resource's keys an
//! untag names (one, several, all, none of them present) — not along the names, which the
//! rules say nothing about.

use std::sync::Arc;

use aws_smithy_types::base64;
use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_greengrassv2_greengrassv2::state::{
    DEPLOYMENT_NOT_FOUND_MESSAGE, MAX_PAGE_SIZE, TAGGING_NOT_FOUND_CODE,
    TAGGING_NOT_FOUND_MESSAGE,
};
use service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for; every minted ARN carries it.
const REGION: &str = "us-west-2";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The status an operation whose modelled output declares no member answers.
const NO_CONTENT: u16 = 204;

/// The `DeploymentStatus` a fresh deployment carries, and the one a cancel moves it to.
const STATUS_ACTIVE: &str = "ACTIVE";
const STATUS_CANCELED: &str = "CANCELED";

/// The code the id-keyed deployment operations model for a deployment that is not there
/// (`smithy_query(query="operation",operation="DeleteDeployment")` lists
/// `ResourceNotFoundException`, HTTP 404).
const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The sentence `DeleteDeployment` answers for a deployment that is still being applied,
/// written here as the literal AWS answers rather than read from the implementation, so a
/// wording that drifts away from it fails this suite.
const MUST_BE_CANCELED_MESSAGE: &str =
    "Invalid Input: Greengrass deployment must be canceled before deletion";

/// The emulator front door with only this service mounted, over ONE shared engine —
/// exactly how `core/router` assembles it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one signed request and answer its status, its `content-length` header (so a
/// no-content answer can be judged on the framing too) and its parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> (u16, Option<String>, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://greengrass.{REGION}.amazonaws.com{path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/greengrass/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let length = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or(Value::String(String::from_utf8_lossy(&raw).into_owned()))
    };
    (status, length, value)
}

/// The status and body alone, for a call whose framing is not what the row is about.
async fn send(router: &MultiServiceRouter, method: &str, path: &str, body: Vec<u8>) -> (u16, Value) {
    let (status, _, value) = call(router, method, path, body).await;
    (status, value)
}

async fn post(router: &MultiServiceRouter, path: &str, body: &Value) -> (u16, Value) {
    let bytes = serde_json::to_vec(body).expect("a serializable request body");
    send(router, "POST", path, bytes).await
}

async fn get(router: &MultiServiceRouter, path: &str) -> (u16, Value) {
    send(router, "GET", path, Vec::new()).await
}

/// A string member of a JSON object, or the empty string when absent.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The wire error code of a `restJson1` error body.
fn error_code(body: &Value) -> String {
    let code = text(body, "__type");
    if code.is_empty() {
        "<no __type>".to_string()
    } else {
        code
    }
}

/// An ARN as a client percent-encodes a non-greedy `@httpLabel`.
fn encoded_label(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// The component-version ARN this emulator mints for `name`@`version`.
fn version_arn(name: &str, version: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:components:{name}:versions:{version}")
}

/// The deployment ARN of `deployment_id`.
fn deployment_arn(deployment_id: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:deployments:{deployment_id}")
}

/// An IoT thing ARN: one of the two target forms a deployment may name.
fn thing_target(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT}:thing/{name}")
}

/// An IoT thing-group ARN: the other target form, and the only one a subdeployment's
/// parent may be.
fn thing_group_target(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT}:thinggroup/{name}")
}

/// `CreateDeployment` for `request`; answers the new deployment's id.
async fn create_deployment(router: &MultiServiceRouter, request: &Value) -> String {
    let (status, created) = post(router, "/greengrass/v2/deployments", request).await;
    assert_eq!(
        status, 201,
        "CreateDeployment {request} must answer 201, got {status} {created}"
    );
    let id = text(&created, "deploymentId");
    assert!(
        !id.is_empty(),
        "CreateDeployment must report a deploymentId, got {created}"
    );
    id
}

/// One deployment for `target`, with no other member set.
async fn deploy_to(router: &MultiServiceRouter, target: &str) -> String {
    create_deployment(router, &json!({ "targetArn": target })).await
}

/// `GetDeployment` on `deployment_id`.
async fn get_deployment(router: &MultiServiceRouter, deployment_id: &str) -> (u16, Value) {
    get(router, &format!("/greengrass/v2/deployments/{deployment_id}")).await
}

/// `CancelDeployment` on `deployment_id`.
async fn cancel_deployment(router: &MultiServiceRouter, deployment_id: &str) -> (u16, Value) {
    post(
        router,
        &format!("/greengrass/v2/deployments/{deployment_id}/cancel"),
        &json!({}),
    )
    .await
}

/// `DeleteDeployment` on `deployment_id`, with the framing of its answer.
async fn delete_deployment(
    router: &MultiServiceRouter,
    deployment_id: &str,
) -> (u16, Option<String>, Value) {
    call(
        router,
        "DELETE",
        &format!("/greengrass/v2/deployments/{deployment_id}"),
        Vec::new(),
    )
    .await
}

/// `ListDeployments` with `query` appended to its path.
async fn list_deployments(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    let path = if query.is_empty() {
        "/greengrass/v2/deployments".to_string()
    } else {
        format!("/greengrass/v2/deployments?{query}")
    };
    get(router, &path).await
}

/// The `deploymentId`s a listing reported, in the order it reported them.
fn listed_ids(body: &Value) -> Vec<String> {
    body.get("deployments")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| text(entry, "deploymentId"))
                .collect()
        })
        .unwrap_or_default()
}

/// The smallest recipe the parser accepts for `name`@`version`.
fn recipe(name: &str, version: &str) -> String {
    format!(
        r#"{{"RecipeFormatVersion":"2020-01-25","ComponentName":"{name}","ComponentVersion":"{version}"}}"#
    )
}

/// Publish `name`@`version` and answer its component-version ARN.
async fn create_component(router: &MultiServiceRouter, name: &str, version: &str) -> String {
    let (status, body) = post(
        router,
        "/greengrass/v2/createComponentVersion",
        &json!({ "inlineRecipe": base64::encode(recipe(name, version).as_bytes()) }),
    )
    .await;
    assert_eq!(
        status, 201,
        "publishing {name}:{version} must answer 201, got {status} {body}"
    );
    version_arn(name, version)
}

/// A tag map as a JSON object.
fn tag_object(tags: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert((*key).to_string(), Value::String((*value).to_string()));
    }
    Value::Object(map)
}

/// `TagResource` on `arn`, with the framing of its answer.
async fn tag(
    router: &MultiServiceRouter,
    arn: &str,
    tags: &[(&str, &str)],
) -> (u16, Option<String>, Value) {
    let body = serde_json::to_vec(&json!({ "tags": tag_object(tags) }))
        .expect("a serializable request body");
    call(router, "POST", &format!("/tags/{}", encoded_label(arn)), body).await
}

/// `UntagResource` on `arn` for `keys` (`DELETE /tags/{resourceArn}?tagKeys=…`), with the
/// framing of its answer.
async fn untag(
    router: &MultiServiceRouter,
    arn: &str,
    keys: &[&str],
) -> (u16, Option<String>, Value) {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={key}"))
        .collect::<Vec<String>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", encoded_label(arn)),
        Vec::new(),
    )
    .await
}

/// `ListTagsForResource` on `arn`: the reader a removal has to be visible in.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    get(router, &format!("/tags/{}", encoded_label(arn))).await
}

// ---------------------------------------------------------------------------
// ListDeployments
// ---------------------------------------------------------------------------

/// RULE: the listing reports the deployments this account recorded, newest first — the
/// latest revision of each target by default, every revision under `ALL` — and each
/// entry's members are the ones that deployment's own reader reports.
#[tokio::test]
async fn the_listing_reports_the_recorded_deployments_newest_first() {
    let router = emulator();

    // The rows differ along the target FORM and the number of revisions a target has: a
    // thing with one revision, a thing group with three, and a subdeployment under a
    // parent thing group.
    let parent = thing_group_target("ParentFleet");
    let single = thing_target("LoneCore");
    let revised = thing_group_target("RevisedFleet");

    let mut created: Vec<(String, String)> = Vec::new();
    created.push((single.clone(), deploy_to(&router, &single).await));
    for revision in 1..=3 {
        let id = create_deployment(
            &router,
            &json!({ "targetArn": revised, "deploymentName": format!("revision-{revision}") }),
        )
        .await;
        created.push((revised.clone(), id));
    }
    let child = thing_group_target("ChildFleet");
    let subdeployment = create_deployment(
        &router,
        &json!({ "targetArn": child, "parentTargetArn": parent }),
    )
    .await;
    created.push((child.clone(), subdeployment.clone()));

    // Newest first, so the creation order reversed.
    let every_id: Vec<String> = created
        .iter()
        .rev()
        .map(|(_, id)| id.clone())
        .collect();
    let latest_per_target: Vec<String> = vec![
        subdeployment.clone(),
        created[3].1.clone(),
        created[0].1.clone(),
    ];

    // (case, query, the ids the listing must report in order)
    let cases = vec![
        (
            "no historyFilter, which defaults to LATEST_ONLY",
            String::new(),
            latest_per_target.clone(),
        ),
        (
            "historyFilter=LATEST_ONLY stated",
            "historyFilter=LATEST_ONLY".to_string(),
            latest_per_target.clone(),
        ),
        (
            "historyFilter=ALL",
            "historyFilter=ALL".to_string(),
            every_id.clone(),
        ),
        (
            "one target's whole history",
            format!("historyFilter=ALL&targetArn={}", encoded_label(&revised)),
            vec![
                created[3].1.clone(),
                created[2].1.clone(),
                created[1].1.clone(),
            ],
        ),
        (
            "one target's latest revision only",
            format!("targetArn={}", encoded_label(&revised)),
            vec![created[3].1.clone()],
        ),
        (
            "the subdeployments of one parent",
            format!("historyFilter=ALL&parentTargetArn={}", encoded_label(&parent)),
            vec![subdeployment.clone()],
        ),
        (
            "a parent nothing is deployed under",
            format!(
                "historyFilter=ALL&parentTargetArn={}",
                encoded_label(&thing_group_target("NoSuchParent"))
            ),
            Vec::new(),
        ),
        (
            "a target nothing is deployed to",
            format!(
                "historyFilter=ALL&targetArn={}",
                encoded_label(&thing_target("NeverDeployed"))
            ),
            Vec::new(),
        ),
    ];

    for (case, query, expected) in cases {
        let (status, body) = list_deployments(&router, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListDeployments must answer 200, got {status} {body}"
        );
        assert_eq!(
            listed_ids(&body),
            expected,
            "case {case:?}: expected exactly {expected:?} in that order, got {body}"
        );
        assert_eq!(
            body.get("nextToken"),
            None,
            "case {case:?}: a listing that fits one page reports no nextToken, got {body}"
        );
    }

    // Every entry carries what the deployment's own reader reports, so the listing is a
    // projection of the stored deployment rather than a second record.
    let (_, listed) = list_deployments(&router, "historyFilter=ALL").await;
    let entries = listed["deployments"]
        .as_array()
        .expect("a listing with entries")
        .clone();
    for entry in entries {
        let id = text(&entry, "deploymentId");
        let (status, stored) = get_deployment(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {id:?}: the listed deployment must be readable, got {status} {stored}"
        );
        for member in [
            "targetArn",
            "revisionId",
            "deploymentId",
            "deploymentStatus",
            "parentTargetArn",
            "deploymentName",
            "creationTimestamp",
        ] {
            assert_eq!(
                entry.get(member),
                stored.get(member),
                "case {id:?}: the listed {member} must be the stored one, got \
                 {:?} and {:?}",
                entry.get(member),
                stored.get(member)
            );
        }
        assert_eq!(
            entry.get("isLatestForTarget"),
            stored.get("isLatestForTarget"),
            "case {id:?}: isLatestForTarget must agree with the reader, got {entry}"
        );
    }
}

/// RULE: a listing is paged by `maxResults` and resumed by the `nextToken` it hands back,
/// so the pages together are exactly the one-page listing, with nothing repeated and
/// nothing dropped.
#[tokio::test]
async fn the_listing_pages_by_max_results_and_resumes_where_it_stopped() {
    let router = emulator();

    let target = thing_group_target("PagedFleet");
    let mut ids: Vec<String> = Vec::new();
    for revision in 1..=5 {
        ids.push(
            create_deployment(
                &router,
                &json!({ "targetArn": target, "deploymentName": format!("r{revision}") }),
            )
            .await,
        );
    }
    ids.reverse();

    // The page sizes span the model's `DefaultMaxResults` range (1..100): its lower
    // bound, sizes inside it, exactly the number stored, more than that, and its upper
    // bound.
    for page_size in [1_i64, 2, 4, 5, 6, i64::from(MAX_PAGE_SIZE)] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= ids.len() + 1,
                "case maxResults={page_size}: paging must terminate, got {pages} pages \
                 for {} deployments",
                ids.len()
            );
            let mut query = format!("historyFilter=ALL&maxResults={page_size}");
            if let Some(cursor) = &token {
                query.push_str(&format!("&nextToken={cursor}"));
            }
            let (status, body) = list_deployments(&router, &query).await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size}: page {pages} must answer 200, got {status} {body}"
            );
            let page = listed_ids(&body);
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= page_size,
                "case maxResults={page_size}: a page carries at most that many entries, \
                 got {} in {body}",
                page.len()
            );
            seen.extend(page);
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, ids,
            "case maxResults={page_size}: the pages together must be the whole history in \
             order, got {seen:?}"
        );
    }

    // One past the upper bound is refused by the modelled range, and a token this service
    // never issued is refused rather than answered from the start of the list.
    for (case, query) in [
        ("maxResults one past the bound", format!("maxResults={}", MAX_PAGE_SIZE + 1)),
        ("maxResults below the bound", "maxResults=0".to_string()),
        ("a nextToken nothing issued", "nextToken=not-a-cursor".to_string()),
    ] {
        let (status, body) = list_deployments(&router, &query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DeleteDeployment
// ---------------------------------------------------------------------------

/// RULE: a deployment is deleted only once it is no longer being applied — an ACTIVE one
/// is refused with the cancel-first sentence and keeps its status — and a deleted
/// deployment is gone from every reader of the surface.
#[tokio::test]
async fn a_deployment_is_deleted_only_after_it_is_cancelled() {
    let router = emulator();

    // The rows differ along the target form and the status the deployment is in when the
    // delete arrives.
    for (case, target) in [
        ("a thing target", thing_target("DeletableCore")),
        ("a thing-group target", thing_group_target("DeletableFleet")),
    ] {
        let id = deploy_to(&router, &target).await;

        // While ACTIVE the delete is refused and nothing changes.
        let (status, _, body) = delete_deployment(&router, &id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: deleting an ACTIVE deployment must answer 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            text(&body, "message"),
            MUST_BE_CANCELED_MESSAGE,
            "case {case:?}: expected the cancel-first sentence, got {body}"
        );
        let (status, stored) = get_deployment(&router, &id).await;
        assert_eq!(
            (status, text(&stored, "deploymentStatus").as_str()),
            (200, STATUS_ACTIVE),
            "case {case:?}: the refused delete must leave the deployment ACTIVE, got \
             {status} {stored}"
        );

        // Cancelled, the delete is served, carries no content, and the deployment is gone
        // from every reader of the surface.
        let (status, cancelled) = cancel_deployment(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the cancel must answer 200, got {status} {cancelled}"
        );
        let (status, stored) = get_deployment(&router, &id).await;
        assert_eq!(
            (status, text(&stored, "deploymentStatus").as_str()),
            (200, STATUS_CANCELED),
            "case {case:?}: the cancelled deployment must read back {STATUS_CANCELED} \
             before the delete, got {status} {stored}"
        );
        let (status, length, body) = delete_deployment(&router, &id).await;
        assert_eq!(
            (status, body.clone()),
            (NO_CONTENT, Value::Null),
            "case {case:?}: the delete must answer {NO_CONTENT} with no body, got \
             {status} {body}"
        );
        assert_eq!(
            length.as_deref(),
            Some("0"),
            "case {case:?}: a no-content answer states the length it has, got {length:?}"
        );

        for (reader, answered) in [
            ("GetDeployment", get_deployment(&router, &id).await),
            ("CancelDeployment", cancel_deployment(&router, &id).await),
        ] {
            let (status, body) = answered;
            assert_eq!(
                status, 404,
                "case {case:?} on {reader}: a deleted deployment must answer 404, got \
                 {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                RESOURCE_NOT_FOUND_CODE,
                "case {case:?} on {reader}: expected {RESOURCE_NOT_FOUND_CODE}, got {body}"
            );
            assert_eq!(
                text(&body, "message"),
                DEPLOYMENT_NOT_FOUND_MESSAGE,
                "case {case:?} on {reader}: expected the surface's fixed sentence, got {body}"
            );
        }
        let (status, _, body) = delete_deployment(&router, &id).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, RESOURCE_NOT_FOUND_CODE),
            "case {case:?}: a second delete must answer 404, got {status} {body}"
        );
        let (_, listed) = list_deployments(&router, "historyFilter=ALL").await;
        assert!(
            !listed_ids(&listed).contains(&id),
            "case {case:?}: a deleted deployment must be gone from the listing, got {listed}"
        );
    }

    // An id nothing ever created, in the forms a client can present one.
    for case in ["00000000-0000-4000-8000-000000000000", "never-created"] {
        let (status, _, body) = delete_deployment(&router, case).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            (error_code(&body), text(&body, "message")),
            (
                RESOURCE_NOT_FOUND_CODE.to_string(),
                DEPLOYMENT_NOT_FOUND_MESSAGE.to_string()
            ),
            "case {case:?}: expected the surface's modelled refusal, got {body}"
        );
    }
}

/// RULE: deleting one revision of a target leaves the rest of that target's history, and
/// the newest survivor is the latest revision for the target.
#[tokio::test]
async fn deleting_a_revision_promotes_the_newest_survivor_for_its_target() {
    let router = emulator();

    let target = thing_group_target("SupersededFleet");
    let mut ids: Vec<String> = Vec::new();
    for revision in 1..=3 {
        ids.push(
            create_deployment(
                &router,
                &json!({ "targetArn": target, "deploymentName": format!("r{revision}") }),
            )
            .await,
        );
    }

    // The newest revision is the latest for the target; cancelling and deleting it leaves
    // the one before it latest.
    let newest = ids.pop().expect("three revisions");
    let (_, stored) = get_deployment(&router, &newest).await;
    assert_eq!(
        stored.get("isLatestForTarget"),
        Some(&Value::Bool(true)),
        "case \"before the delete\": the newest revision is latest for its target, got {stored}"
    );
    let (status, cancelled) = cancel_deployment(&router, &newest).await;
    assert_eq!(
        status, 200,
        "case \"the cancel\": expected 200, got {status} {cancelled}"
    );
    let (status, _, body) = delete_deployment(&router, &newest).await;
    assert_eq!(
        status, NO_CONTENT,
        "case \"the delete\": expected {NO_CONTENT}, got {status} {body}"
    );

    let promoted = ids.last().expect("two revisions left").clone();
    let (status, stored) = get_deployment(&router, &promoted).await;
    assert_eq!(
        (status, stored.get("isLatestForTarget")),
        (200, Some(&Value::Bool(true))),
        "case \"the survivor\": the newest surviving revision is latest for its target, \
         got {status} {stored}"
    );
    let (_, latest_only) = list_deployments(
        &router,
        &format!("targetArn={}", encoded_label(&target)),
    )
    .await;
    assert_eq!(
        listed_ids(&latest_only),
        vec![promoted.clone()],
        "case \"the default listing\": only the survivor is latest for the target, got \
         {latest_only}"
    );
    let (_, whole_history) = list_deployments(
        &router,
        &format!("historyFilter=ALL&targetArn={}", encoded_label(&target)),
    )
    .await;
    let mut expected = ids.clone();
    expected.reverse();
    assert_eq!(
        listed_ids(&whole_history),
        expected,
        "case \"the whole history\": every surviving revision is still listed, got \
         {whole_history}"
    );
}

// ---------------------------------------------------------------------------
// UntagResource
// ---------------------------------------------------------------------------

/// One row of the untag table: the keys one request removes, and the tag set the resource
/// must be left holding.
struct UntagCase {
    case: &'static str,
    /// The `tagKeys` the request names.
    keys: &'static [&'static str],
    /// What `ListTagsForResource` must report afterwards.
    remaining: &'static [(&'static str, &'static str)],
}

/// RULE: an untag removes exactly the keys it names from the tag set the resource itself
/// holds, leaves every other key, and is idempotent in a key the resource does not carry.
#[tokio::test]
async fn untagging_removes_exactly_the_keys_it_names() {
    let router = emulator();

    // The rows differ along how many of the resource's keys the request names, and whether
    // they are keys it carries.
    let rows: [UntagCase; 5] = [
        UntagCase {
            case: "one of three keys",
            keys: &["env"],
            remaining: &[("owner", "fleet-team"), ("tier", "edge")],
        },
        UntagCase {
            case: "two of three keys",
            keys: &["env", "tier"],
            remaining: &[("owner", "fleet-team")],
        },
        UntagCase {
            case: "every key",
            keys: &["env", "owner", "tier"],
            remaining: &[],
        },
        UntagCase {
            case: "a key the resource does not carry",
            keys: &["absent"],
            remaining: &[("env", "prod"), ("owner", "fleet-team"), ("tier", "edge")],
        },
        UntagCase {
            case: "one key it carries and one it does not",
            keys: &["env", "absent"],
            remaining: &[("owner", "fleet-team"), ("tier", "edge")],
        },
    ];
    let written: [(&str, &str); 3] = [
        ("env", "prod"),
        ("owner", "fleet-team"),
        ("tier", "edge"),
    ];

    for (row, UntagCase { case, keys, remaining }) in rows.iter().enumerate() {
        // Both resource kinds this service holds a tag set for, so the rule is the
        // resource's and not one kind's.
        let component = create_component(&router, &format!("com.example.Untag{row}"), "1.0.0").await;
        let deployment =
            deployment_arn(&deploy_to(&router, &thing_target(&format!("UntagCore{row}"))).await);

        for (kind, arn) in [("a component version", &component), ("a deployment", &deployment)] {
            let (status, _, body) = tag(&router, arn, &written).await;
            assert_eq!(
                status, NO_CONTENT,
                "case {case:?} on {kind}: the tag write must answer {NO_CONTENT}, got \
                 {status} {body}"
            );

            // The removal, twice: the second call asks for a tag set the resource already
            // has, which is not a failure and changes nothing.
            for attempt in 1..=2 {
                let (status, length, body) = untag(&router, arn, keys).await;
                assert_eq!(
                    (status, body.clone()),
                    (NO_CONTENT, Value::Null),
                    "case {case:?} on {kind}, attempt {attempt}: expected {NO_CONTENT} with \
                     no body, got {status} {body}"
                );
                assert_eq!(
                    length.as_deref(),
                    Some("0"),
                    "case {case:?} on {kind}, attempt {attempt}: a no-content answer states \
                     the length it has, got {length:?}"
                );

                let (status, listed) = list_tags(&router, arn).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} on {kind}, attempt {attempt}: the listing must answer 200, \
                     got {status} {listed}"
                );
                assert_eq!(
                    listed["tags"],
                    tag_object(remaining),
                    "case {case:?} on {kind}, attempt {attempt}: expected exactly the keys \
                     the removal left, got {listed}"
                );
            }
        }
    }
}

/// RULE: an untag of an ARN that names no resource this service holds is refused with the
/// tagging route's OWN not-found class — the same code and sentence the write and the read
/// of a resource's tags answer — and nothing is written.
#[tokio::test]
async fn untagging_an_arn_that_names_nothing_is_the_tagging_routes_own_refusal() {
    let router = emulator();

    // One real resource of each kind, so no row below is refused because the store is
    // empty.
    let present = create_component(&router, "com.example.Present", "1.0.0").await;
    tag(&router, &present, &[("env", "prod")]).await;

    // The ways a well-formed `GenericV2ARN` can name nothing: a version never published,
    // a deployment never created, and a core device nothing registers.
    for (case, arn) in [
        (
            "a component version never published",
            version_arn("com.example.NeverPublished", "1.0.0"),
        ),
        (
            "a version of a published component that was not",
            version_arn("com.example.Present", "9.9.9"),
        ),
        (
            "a deployment never created",
            deployment_arn("00000000-0000-4000-8000-000000000000"),
        ),
        (
            "a core device nothing registers",
            format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:coreDevices:NeverCheckedIn"),
        ),
    ] {
        let (status, _, body) = untag(&router, &arn, &["env"]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            TAGGING_NOT_FOUND_CODE,
            "case {case:?}: expected the tagging route's own code, got {body}"
        );
        assert_eq!(
            text(&body, "message"),
            TAGGING_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the tagging route's fixed sentence, got {body}"
        );
    }

    // The live resource still untags, so every row above was refused for what it named
    // and not because the route stopped working.
    let (status, _, body) = untag(&router, &present, &["env"]).await;
    assert_eq!(
        status, NO_CONTENT,
        "case \"the published component version\": the live resource must still untag, \
         got {status} {body}"
    );
    let (status, listed) = list_tags(&router, &present).await;
    assert_eq!(
        (status, listed["tags"].clone()),
        (200, tag_object(&[])),
        "case \"the published component version\": the removal must be readable, got \
         {status} {listed}"
    );
}
