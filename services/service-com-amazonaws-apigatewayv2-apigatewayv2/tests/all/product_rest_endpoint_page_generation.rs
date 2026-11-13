// SPDX-License-Identifier: CC0-1.0

//! A product REST endpoint page documents a REST endpoint the ACCOUNT HAS, and the write is
//! what resolves it.
//!
//! The rules these cases assert:
//!
//! > `CreateProductRestEndpointPage` and `UpdateProductRestEndpointPage` generate the page's
//! > content from the REST endpoint its `RestEndpointIdentifier` names — a REST API of the
//! > API Gateway service, under one of that API's stages. A write naming an endpoint the
//! > account does NOT have is refused 400 `BadRequestException` with the sentence
//! > `Invalid restEndpointIdentifier stage or restApiId`, and persists nothing. A write
//! > whose endpoint resolves answers with a page that reports `IN_PROGRESS` until the
//! > generation comes due and `AVAILABLE` after it, carrying no `StatusException`.
//! >
//! > The resolution belongs to the WRITE: a REST API or stage that comes or goes afterwards
//! > does not move an already-stored page until the next write resolves the endpoint again.
//!
//! Every REST API and stage a row names was created through the service that OWNS them
//! (`apigateway:CreateRestApi`, `apigateway:PutMethod`, `apigateway:CreateDeployment`),
//! mounted beside this one on the same shared engine exactly as the router mounts them, so
//! nothing here passes off an identifier of its own as one the account holds.
//!
//! The rows differ along what the rule generalises over: whether the account holds the
//! named REST API at all, whether it holds the named STAGE of an API it does hold, which
//! method and path of that API the page documents (neither takes part in the resolution),
//! and whether the endpoint came to exist before or after the write.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::ComAmazonawsApigatewayv2Apigatewayv2State;
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{
    as_str, call, carries_nothing, emulator_with_rest_apis_and_engine, error_code, read_until,
    rest_endpoint,
};
use crate::product_rest_endpoint_page_lifecycle::{invoke_url, pages_of, product, REGION};

/// This service and the API Gateway (v1) service that owns REST APIs, on ONE shared engine
/// — the arrangement the router assembles, and the one the resolution under test reads the
/// account's REST APIs through.
///
/// `product_rest_endpoint_page_published_endpoint` drives the same arrangement, so the
/// builder is shared from here; both delegate to `api_lifecycle`'s, which every page suite of
/// this crate uses.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_rest_apis_and_engine().0
}

/// [`emulator`] and the engine both services were built from, so a case can assert on the
/// store itself: what a refused write must NOT have persisted.
/// `ListProductRestEndpointPages` is still a scaffold stub (its own open issue owns it), so a
/// product's pages are counted here rather than read over the wire.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    emulator_with_rest_apis_and_engine()
}

/// Read one page back once its content generation has SETTLED, asserting the read
/// succeeded: the wait is for the page to stop reporting the transient status, never for a
/// fixed time (`api_lifecycle::read_until`).
async fn settled_page(emulator: &MultiServiceRouter, case: &str, uri: &str) -> Value {
    read_until(emulator, case, uri, |page| {
        page.get("status").and_then(Value::as_str) != Some(IN_PROGRESS)
    })
    .await
}

/// The status a page reports while its content is being generated.
const IN_PROGRESS: &str = "IN_PROGRESS";

/// API Gateway's own sentence for a `restEndpointIdentifier` that names no stage or no REST
/// API — every part of it fixed text, naming neither the API nor the stage.
const UNRESOLVED_MESSAGE: &str = "Invalid restEndpointIdentifier stage or restApiId";

/// Create one REST API with `stage` deployed on it, through the service that owns REST
/// APIs, and answer its `RestApiId` — the shared helper every page suite of this crate
/// creates an endpoint through.
async fn rest_api_with_stage(emulator: &MultiServiceRouter, name: &str, stage: &str) -> String {
    rest_endpoint(emulator, name, stage).await
}

/// Deploy `stage` on `rest_api_id` through the service that owns REST APIs, and read the
/// stage back there: what this suite calls "a stage the account holds" is what that service
/// reports, not what this one assumed.
async fn deploy_stage(emulator: &MultiServiceRouter, rest_api_id: &str, stage: &str) {
    let (status, deployment) = call(
        emulator,
        "POST",
        &format!("/restapis/{rest_api_id}/deployments"),
        Some(json!({ "stageName": stage })),
    )
    .await;
    assert_eq!(
        status, 201,
        "stage {stage:?}: expected 201 from CreateDeployment, got {status} {deployment}"
    );
    let (status, read) = call(
        emulator,
        "GET",
        &format!("/restapis/{rest_api_id}/stages/{stage}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "stage {stage:?}: the owner of REST APIs must report the stage it was asked to \
         create, got {status} {read}"
    );
}

/// Take `stage` of `rest_api_id` away again, there too, and confirm it is gone.
async fn delete_stage(emulator: &MultiServiceRouter, rest_api_id: &str, stage: &str) {
    let uri = format!("/restapis/{rest_api_id}/stages/{stage}");
    let (status, body) = call(emulator, "DELETE", &uri, None).await;
    assert_eq!(
        status, 202,
        "stage {stage:?}: expected the owner of REST APIs to accept DeleteStage, got \
         {status} {body}"
    );
    let (status, body) = call(emulator, "GET", &uri, None).await;
    assert_eq!(
        status, 404,
        "stage {stage:?}: expected the deleted stage to resolve to nothing, got {status} \
         {body}"
    );
}

/// One `CreateProductRestEndpointPage` documenting `parts`.
async fn create_page(
    emulator: &MultiServiceRouter,
    collection: &str,
    parts: Value,
) -> (u16, Value) {
    call(
        emulator,
        "POST",
        collection,
        Some(json!({ "restEndpointIdentifier": { "identifierParts": parts } })),
    )
    .await
}

/// Read one page back, asserting the read succeeded.
async fn read(emulator: &MultiServiceRouter, case: &str, uri: &str) -> Value {
    let (status, body) = call(emulator, "GET", uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetProductRestEndpointPage, got {status} {body}"
    );
    body
}

/// Assert `document` reports `expected_status` and, whichever it is, no `StatusException`:
/// every page this service stores documents an endpoint that resolved, so there is never
/// anything to explain.
fn assert_settled(case: &str, document: &Value, expected_status: &str) {
    assert_eq!(
        as_str(document, "status"),
        expected_status,
        "case {case:?}: expected the page to report {expected_status}, got {document}"
    );
    assert!(
        carries_nothing(document, "statusException"),
        "case {case:?}: a page whose endpoint resolved has nothing to explain, got {document}"
    );
}

/// Assert one write was refused for the endpoint it named: the status, the modelled error
/// and the one sentence API Gateway states.
fn assert_refused(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 for an endpoint the account does not have, got \
         {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "BadRequestException",
        "case {case:?}: expected BadRequestException, got {body}"
    );
    assert_eq!(
        as_str(body, "message"),
        UNRESOLVED_MESSAGE,
        "case {case:?}: expected the unresolved-endpoint sentence, got {body}"
    );
}

/// How many product REST endpoint pages `portal_product_id` holds.
fn page_count(engine: &engine::InMemoryEngine, portal_product_id: &str) -> usize {
    engine
        .state::<ComAmazonawsApigatewayv2Apigatewayv2State>()
        .lock()
        .portal_products
        .get(portal_product_id)
        .map_or(0, |product| product.product_rest_endpoint_pages.len())
}

/// One row: the REST endpoint a page documents, and whether the write is served.
struct Row {
    case: &'static str,
    /// The stage to deploy on the row's own REST API, or `None` for a row whose page names
    /// a REST API the account holds nothing under.
    deployed_stage: Option<&'static str>,
    /// The `stage` the page's identifier names.
    identified_stage: &'static str,
    /// The `method` and `path` the page's identifier names: the endpoint of the API it
    /// documents, which takes no part in whether the API and stage resolve.
    method: &'static str,
    path: &'static str,
    /// Whether the account HAS the endpoint the identifier names, so the write is served.
    held: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a REST API and a stage the account holds",
            deployed_stage: Some("prod"),
            identified_stage: "prod",
            method: "GET",
            path: "/orders",
            held: true,
        },
        Row {
            case: "another endpoint of that same stage: the method and path take no part",
            deployed_stage: Some("prod"),
            identified_stage: "prod",
            method: "POST",
            path: "/accounts/{accountId}/orders/{proxy+}",
            held: true,
        },
        Row {
            case: "a stage name the account's REST API has no stage under",
            deployed_stage: Some("prod"),
            identified_stage: "beta",
            method: "GET",
            path: "/orders",
            held: false,
        },
        Row {
            case: "a REST API id the account holds nothing under",
            deployed_stage: None,
            identified_stage: "prod",
            method: "GET",
            path: "/orders",
            held: false,
        },
        Row {
            case: "a REST API the account holds and a stage named like its default one",
            deployed_stage: Some("v1"),
            identified_stage: "$default",
            method: "DELETE",
            path: "/orders/{orderId}",
            held: false,
        },
    ]
}

#[tokio::test]
async fn a_write_is_served_only_for_an_endpoint_the_account_has() {
    let (emulator, engine) = emulator_with_engine();
    let product_id = product(&emulator, "Generation").await;
    let collection = pages_of(&product_id);

    // A REST API id the account holds nothing under, in the form the owner of REST APIs
    // mints (ten lowercase alphanumerics), so what fails is the resolution and not the
    // shape of the identifier.
    const UNKNOWN_REST_API: &str = "nosuchapi0";

    for row in rows() {
        let case = row.case;
        let rest_api_id = match row.deployed_stage {
            Some(stage) => rest_api_with_stage(&emulator, case, stage).await,
            None => UNKNOWN_REST_API.to_string(),
        };
        let parts = json!({
            "restApiId": rest_api_id,
            "stage": row.identified_stage,
            "method": row.method,
            "path": row.path
        });

        let held_before = page_count(&engine, &product_id);
        let (status, created) = create_page(&emulator, &collection, parts).await;
        if !row.held {
            assert_refused(case, status, &created);
            assert_eq!(
                page_count(&engine, &product_id),
                held_before,
                "case {case:?}: a refused create must persist no page"
            );
            continue;
        }
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateProductRestEndpointPage, got {status} \
             {created}"
        );
        let page_id = as_str(&created, "productRestEndpointPageId");
        let uri = format!("{collection}/{page_id}");

        // The content is still being generated when the create answers, so a caller acting
        // straight after it sees the transient status.
        assert_settled(case, &created, IN_PROGRESS);

        // The endpoint a page reports is the invoke URL of the STAGE its identifier names.
        assert_eq!(
            created
                .get("displayContent")
                .and_then(|content| content.get("endpoint"))
                .and_then(Value::as_str),
            Some(invoke_url(&rest_api_id, row.identified_stage, REGION).as_str()),
            "case {case:?}: expected the stage invoke URL, got {created}"
        );

        let settled = settled_page(&emulator, case, &uri).await;
        assert_settled(case, &settled, "AVAILABLE");

        // A read reports: two reads of one settled page answer the same record.
        let again = read(&emulator, case, &uri).await;
        assert_eq!(
            again, settled,
            "case {case:?}: reading a settled page must not move it, got {again} after \
             {settled}"
        );
    }
}

#[tokio::test]
async fn the_endpoint_is_resolved_by_the_write_and_a_later_write_resolves_it_again() {
    let (emulator, _engine) = emulator_with_engine();
    let product_id = product(&emulator, "Re-generation").await;
    let collection = pages_of(&product_id);

    // One REST API with one stage, both the account's own, and the stage name it does NOT
    // hold a stage under yet.
    let rest_api_id = rest_api_with_stage(&emulator, "the account's REST API", "prod").await;
    let parts = |stage: &str| {
        json!({
            "restApiId": rest_api_id,
            "stage": stage,
            "method": "GET",
            "path": "/orders"
        })
    };

    // A page whose stage the API has none of: nothing to generate its content from, so the
    // create is refused.
    let late = "a page whose stage was not deployed when it was written";
    let (status, refused) = create_page(&emulator, &collection, parts("beta")).await;
    assert_refused(late, status, &refused);

    // A page whose stage the API already has: its content is generated from a real
    // endpoint.
    let early = "a page created while the stage it documents was deployed";
    let (status, created) = create_page(&emulator, &collection, parts("prod")).await;
    assert_eq!(
        status, 201,
        "case {early:?}: expected 201 from CreateProductRestEndpointPage, got {status} \
         {created}"
    );
    let early_uri = format!("{collection}/{}", as_str(&created, "productRestEndpointPageId"));
    let early_before = settled_page(&emulator, early, &early_uri).await;
    assert_settled(early, &early_before, "AVAILABLE");

    // Now the account's REST API gains the stage it lacked and loses the one it had. The
    // stored page was resolved by the write that generated its content, so it does NOT
    // move: nothing re-generated it.
    deploy_stage(&emulator, &rest_api_id, "beta").await;
    delete_stage(&emulator, &rest_api_id, "prod").await;
    assert_eq!(
        read(&emulator, early, &early_uri).await,
        early_before,
        "case {early:?}: a REST API that moved after the write must not move the page"
    );

    // The create the account could not serve before is served now, against the endpoint as
    // it is NOW.
    let (status, created_late) = create_page(&emulator, &collection, parts("beta")).await;
    assert_eq!(
        status, 201,
        "case {late:?}: expected 201 once the stage is deployed, got {status} {created_late}"
    );
    let late_uri = format!(
        "{collection}/{}",
        as_str(&created_late, "productRestEndpointPageId")
    );
    assert_settled(
        late,
        &settled_page(&emulator, late, &late_uri).await,
        "AVAILABLE",
    );

    // And the page whose stage is gone cannot be patched: a patch RE-GENERATES the content
    // from the endpoint the page documents, which the account no longer has. The refused
    // patch leaves the stored page exactly as it was.
    let (status, patched) = call(
        &emulator,
        "PATCH",
        &early_uri,
        Some(json!({ "tryItState": "ENABLED" })),
    )
    .await;
    assert_refused(early, status, &patched);
    assert_eq!(
        read(&emulator, early, &early_uri).await,
        early_before,
        "case {early:?}: a refused patch must leave the stored page unchanged"
    );
}
