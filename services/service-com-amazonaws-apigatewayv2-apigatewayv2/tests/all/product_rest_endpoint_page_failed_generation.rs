// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `Status` a product REST endpoint page SETTLES on, and for
//! the `StatusException` the failing one carries.
//!
//! The rule these cases assert, one level above the recorded lifecycle (a page documenting an
//! api of this service, read back twice):
//!
//! > A product REST endpoint page's content is the documentation of the REST API its
//! > `RestEndpointIdentifier` names ("By default, this is the documentation of your REST API
//! > from API Gateway", `DisplayContentOverrides$Body`), and only the service that OWNS REST
//! > APIs holds that documentation. So the page's `Status` is DERIVED from what the write
//! > resolved: `IN_PROGRESS` while the generation is due, then the terminal `AVAILABLE` for a
//! > page whose identifier names a REST API and stage that service holds, and the terminal
//! > `FAILED` for one whose identifier names an api the account has in another control plane
//! > — an HTTP or WebSocket API of this service, which has no REST API documentation to
//! > generate from. A `FAILED` page carries `StatusException` with both members populated,
//! > explaining the page's OWN unresolved endpoint reference; every other status carries none.
//! > A read never moves a page, and a patch starts the generation over and settles it the same
//! > way.
//!
//! WHAT those two members say is `product_rest_endpoint_page_status_exception`'s subject; this
//! suite asserts WHICH status a page settles on and that the exception travels with — and only
//! with — the status that says the page could not be assembled. This suite therefore no longer
//! asserts that the message NAMES the identifier's `restApiId`: the sentence this service states
//! for a `RestEndpointIdentifier` that identifies no REST endpoint has no variable parts — API
//! Gateway names neither the API nor the stage in it — so a page reporting that failure cannot
//! carry the id, and the member's content is asserted in the suite that owns it.
//!
//! The rows differ along what the rule generalises over: which control plane holds the api the
//! identifier names (the REST one, and this service's own for each protocol type it serves),
//! how many stages that api carries and which of them is named, and the method and path the
//! page documents — which select WHICH endpoint of the api the page is of and so take no part
//! in where its documentation lives.
//!
//! Every api a row names is created through the service that OWNS it, both mounted on one
//! shared engine exactly as the router mounts them, so no case passes off an identifier of its
//! own as one the account holds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{
    as_str, call, carries_nothing, emulator_with_rest_apis, read_until, rest_endpoint,
};
use crate::product_rest_endpoint_page_lifecycle::{pages_of, product};

/// The status a page reports while its content is still being generated — the one status that
/// is not terminal, so it is what a case waits to stop seeing.
const IN_PROGRESS: &str = "IN_PROGRESS";

/// The terminal status of a page whose content was generated.
const AVAILABLE: &str = "AVAILABLE";

/// The terminal status of a page whose content could not be generated — the status the
/// modelled `StatusException` explains.
const FAILED: &str = "FAILED";

/// The modelled bounds of `StatusException$Exception` (`__stringMin1Max256`) and
/// `StatusException$Message` (`__stringMin1Max2048`), from
/// `smithy_query(query="shape", shape="StatusException")`: a populated member fits inside
/// them, so neither the empty string nor an unbounded text satisfies the rule.
const EXCEPTION_BOUNDS: (usize, usize) = (1, 256);
const MESSAGE_BOUNDS: (usize, usize) = (1, 2048);

/// Where the api an identifier names lives, which is what decides whether the documentation a
/// page is generated from exists at all.
enum Holder {
    /// A REST API of the API Gateway control plane that owns REST APIs, with one stage
    /// deployed on it.
    RestControlPlane,
    /// An api of THIS service: the `CreateApi` request and the stages to put on it. Such an
    /// api is one the account really has — it shares the account's api-id space — and it is
    /// not a REST API, so no REST API documentation can be generated for it.
    OwnControlPlane { request: Value, stages: Vec<String> },
}

/// One row: where the identified api lives, which of its stages the page documents, the method
/// and path it documents, and the status the page must therefore settle on.
struct Row {
    case: &'static str,
    holder: Holder,
    /// Which of the holder's stages the identifier names, by index.
    stage_index: usize,
    method: &'static str,
    path: &'static str,
    settles: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a REST API of the service that owns REST APIs, with the stage it deployed",
            holder: Holder::RestControlPlane,
            stage_index: 0,
            method: "GET",
            path: "/items",
            settles: AVAILABLE,
        },
        Row {
            case: "another endpoint of that same REST stage: the method and path take no part",
            holder: Holder::RestControlPlane,
            stage_index: 0,
            method: "DELETE",
            path: "/orders/{orderId}",
            settles: AVAILABLE,
        },
        Row {
            case: "an HTTP api of this service, which holds no REST API documentation",
            holder: Holder::OwnControlPlane {
                request: json!({ "name": "failed-generation-http", "protocolType": "HTTP" }),
                stages: vec!["live".to_string()],
            },
            stage_index: 0,
            method: "GET",
            path: "/items",
            settles: FAILED,
        },
        Row {
            case: "the third stage of an HTTP api of this service, not its first",
            holder: Holder::OwnControlPlane {
                request: json!({ "name": "failed-generation-stages", "protocolType": "HTTP" }),
                stages: vec!["dev".to_string(), "beta".to_string(), "prod".to_string()],
            },
            stage_index: 2,
            method: "POST",
            path: "/accounts/{accountId}/orders/{proxy+}",
            settles: FAILED,
        },
        Row {
            case: "a WEBSOCKET api of this service",
            holder: Holder::OwnControlPlane {
                request: json!({
                    "name": "failed-generation-socket",
                    "protocolType": "WEBSOCKET",
                    "routeSelectionExpression": "$request.body.action"
                }),
                stages: vec!["ws".to_string()],
            },
            stage_index: 0,
            method: "ANY",
            path: "/",
            settles: FAILED,
        },
    ]
}

/// Create the api a row's page documents through the service that OWNS it, and answer
/// `(restApiId, stage)` as the identifier must name them — the id that service MINTED, never
/// one a case made up.
async fn held(emulator: &MultiServiceRouter, row: &Row, index: usize) -> (String, String) {
    match &row.holder {
        Holder::RestControlPlane => {
            let stage = "prod".to_string();
            let rest_api_id =
                rest_endpoint(emulator, &format!("settled-rest-{index}"), &stage).await;
            (rest_api_id, stage)
        }
        Holder::OwnControlPlane { request, stages } => {
            let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
            assert_eq!(
                status, 201,
                "case {:?}: expected 201 from CreateApi, got {status} {created}",
                row.case
            );
            let api_id = as_str(&created, "apiId");
            for stage in stages {
                let (status, body) = call(
                    emulator,
                    "POST",
                    &format!("/v2/apis/{api_id}/stages"),
                    Some(json!({ "stageName": stage })),
                )
                .await;
                assert_eq!(
                    status, 201,
                    "case {:?}: expected 201 from CreateStage for {stage:?}, got {status} \
                     {body}",
                    row.case
                );
            }
            (api_id, stages[row.stage_index].clone())
        }
    }
}

/// The `identifierParts` of one endpoint, as a request writes them.
fn parts(rest_api_id: &str, stage: &str, row: &Row) -> Value {
    json!({
        "restApiId": rest_api_id,
        "stage": stage,
        "method": row.method,
        "path": row.path
    })
}

/// Read `uri` until the page stopped reporting the transient status: the wait is for that
/// CONDITION, never for a fixed time.
async fn settled(emulator: &MultiServiceRouter, case: &str, uri: &str) -> Value {
    read_until(emulator, case, uri, |page| {
        page.get("status").and_then(Value::as_str) != Some(IN_PROGRESS)
    })
    .await
}

/// Assert `page` reports `expected` and carries exactly the `StatusException` that status
/// takes: both members populated and within the modelled bounds for the failing status, and
/// nothing at all for any other.
fn assert_status(case: &str, what: &str, page: &Value, expected: &str) {
    let reported = as_str(page, "status");
    assert_eq!(
        reported, expected,
        "case {case:?}: expected {what} to report {expected}, got {reported:?} in {page}"
    );
    if expected != FAILED {
        assert!(
            carries_nothing(page, "statusException"),
            "case {case:?}: expected {what} to carry no statusException with status \
             {expected}, got {page}"
        );
        return;
    }
    let exception = page
        .pointer("/statusException/exception")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let message = page
        .pointer("/statusException/message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (exception_min, exception_max) = EXCEPTION_BOUNDS;
    assert!(
        (exception_min..=exception_max).contains(&exception.chars().count()),
        "case {case:?}: expected {what} to populate statusException.exception within \
         {EXCEPTION_BOUNDS:?}, got {exception:?} in {page}"
    );
    let (message_min, message_max) = MESSAGE_BOUNDS;
    assert!(
        (message_min..=message_max).contains(&message.chars().count()),
        "case {case:?}: expected {what} to populate statusException.message within \
         {MESSAGE_BOUNDS:?}, got {message:?} in {page}"
    );
}

#[tokio::test]
async fn a_page_settles_on_what_the_documentation_it_is_generated_from_allows() {
    let emulator = emulator_with_rest_apis();
    let product_id = product(&emulator, "Settled pages").await;
    let collection = pages_of(&product_id);

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let (rest_api_id, stage) = held(&emulator, &row, index).await;

        // Every row's create is SERVED: the account really has the api and stage the
        // identifier names, whichever control plane holds it.
        let (status, created) = call(
            &emulator,
            "POST",
            &collection,
            Some(json!({
                "restEndpointIdentifier": { "identifierParts": parts(&rest_api_id, &stage, &row) }
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateProductRestEndpointPage, got {status} \
             {created}"
        );
        let uri = format!(
            "{collection}/{}",
            as_str(&created, "productRestEndpointPageId")
        );

        // The content is still being generated when the create answers, so a caller acting
        // straight after it sees the transient status — and nothing is explained yet.
        assert_status(
            case,
            "CreateProductRestEndpointPage",
            &created,
            IN_PROGRESS,
        );

        // Then the page settles on what its documentation allows, and reports the exception
        // that says why when it could not be generated.
        let read = settled(&emulator, case, &uri).await;
        assert_status(case, "GetProductRestEndpointPage", &read, row.settles);

        // A read REPORTS: a second read of a settled page answers the same record, so neither
        // the status nor its exception is produced by being read.
        let again = settled(&emulator, case, &uri).await;
        assert_eq!(
            again, read,
            "case {case:?}: reading a settled page must not move it, got {again} after {read}"
        );

        // A patch RE-GENERATES the content from the same api, so the page goes back through
        // the transient status and settles on the same terminal one.
        let (status, patched) = call(
            &emulator,
            "PATCH",
            &uri,
            Some(json!({ "tryItState": "ENABLED" })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateProductRestEndpointPage, got {status} \
             {patched}"
        );
        assert_status(
            case,
            "UpdateProductRestEndpointPage",
            &patched,
            IN_PROGRESS,
        );
        let resettled = settled(&emulator, case, &uri).await;
        assert_status(case, "the patched page", &resettled, row.settles);
    }
}

#[tokio::test]
async fn the_raw_content_reader_reports_the_same_settled_status() {
    let emulator = emulator_with_rest_apis();
    let product_id = product(&emulator, "Raw readings").await;
    let collection = pages_of(&product_id);

    // One page over an api of this service (no REST API documentation to generate from) and
    // one over a REST API of the service that owns them, so both terminal statuses are read
    // through the reader that asks for the page's raw content as well.
    let (status, api) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "raw-reading-http", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(status, 201, "expected 201 from CreateApi, got {status} {api}");
    let own_api_id = as_str(&api, "apiId");
    let (status, stage) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{own_api_id}/stages"),
        Some(json!({ "stageName": "live" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateStage, got {status} {stage}"
    );
    let rest_api_id = rest_endpoint(&emulator, "raw-reading-rest", "prod").await;

    for (case, api_id, stage, expected) in [
        ("an api of this service", own_api_id, "live", FAILED),
        ("a REST API of its owner", rest_api_id, "prod", AVAILABLE),
    ] {
        let (status, created) = call(
            &emulator,
            "POST",
            &collection,
            Some(json!({
                "restEndpointIdentifier": {
                    "identifierParts": {
                        "restApiId": api_id,
                        "stage": stage,
                        "method": "GET",
                        "path": "/items"
                    }
                }
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateProductRestEndpointPage, got {status} \
             {created}"
        );
        let uri = format!(
            "{collection}/{}?includeRawDisplayContent=true",
            as_str(&created, "productRestEndpointPageId")
        );
        let read = settled(&emulator, case, &uri).await;
        assert_status(
            case,
            "GetProductRestEndpointPage(includeRawDisplayContent)",
            &read,
            expected,
        );
    }
}
