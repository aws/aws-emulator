// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ADDRESS a product REST endpoint page publishes for the
//! REST stage it documents, and for whether that address is one this emulator answers on.
//!
//! The rule, one level above the recorded page that filed the issue (a published page whose
//! `displayContent.endpoint` named a host nothing here serves):
//!
//! > An address this service publishes for a consumer to DIAL is the address this emulator
//! > itself serves that resource at, chosen by the endpoint the writing request reached: a
//! > client that reached the emulator's own endpoint is given that endpoint with the
//! > resource behind the reserved data-plane path, and a client that resolves AWS's own
//! > hosts is given the resource's documented host. For a page, that resource is the REST
//! > stage its `RestEndpointIdentifier` names ("The URL to invoke your REST API",
//! > `EndpointDisplayContentResponse$Endpoint`), so every operation that reports the page
//! > publishes that one address and a request to it reaches the stage's integration.
//!
//! The rows span what the rule generalises over: the endpoint the writing request was
//! addressed at (a loopback literal with a port, a loopback NAME on another port, and one of
//! AWS's own service hosts), each signed for a different Region, with a different stage name
//! and a different documented method and path. Each row is driven through every operation
//! that reports a page (`CreateProductRestEndpointPage`, `GetProductRestEndpointPage`,
//! `UpdateProductRestEndpointPage`), and the published address is then DIALED: the page
//! endpoint is only a fix if the stage answers on it.
//!
//! The REST API, its `MOCK` integration and its stage are created through the service that
//! OWNS them (`apigateway:CreateRestApi`, `apigateway:PutMethod`,
//! `apigateway:PutIntegration`, `apigateway:CreateDeployment`), mounted beside this one on
//! the same shared engine exactly as the router mounts them, so the address asserted here is
//! served by the owner of REST APIs and not by anything this suite set up itself.
//!
//! An endpoint the request OVERRODE is the caller's own value and is reported as given: that
//! is the last row of the update table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call_at_host};
use crate::product_rest_endpoint_page_generation::emulator;
use crate::product_rest_endpoint_page_lifecycle::pages_of;
use crate::websocket_handshake_data_plane::send_with_body;

/// The front door's envelope for a request no service claimed. The defect this suite pins is
/// a published address nothing serves, so this is never a legitimate answer here.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// The body the documented stage's `MOCK` integration answers, which is what proves a dialed
/// address reached THAT stage's integration rather than any other answer with a 200 on it.
const SERVED_BODY: &str = r#"{"served":true}"#;

/// The content type the stage's integration request template is keyed by, which a probe of
/// the published address therefore declares.
const JSON: &str = "application/json";

/// Which address a row's page must publish.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Announced {
    /// The emulator's own endpoint, with the REST API behind the reserved data-plane path
    /// (`/_aws/execute-api/{api-id}/{stage}`) — the addressing the API Gateway (v1) data
    /// plane of this emulator serves.
    EmulatorEndpoint,
    /// The `{api-id}.execute-api.{region}.amazonaws.com` host AWS publishes for a REST API,
    /// which that data plane claims host-style.
    ExecuteApiHost,
}

/// One row: the endpoint the writing requests are addressed at, the Region they are signed
/// for, the stage the documented endpoint is deployed to, the method and path the page
/// documents, and the address the page must publish.
struct Row {
    case: &'static str,
    /// The `Host` every control-plane request of this row carries — the endpoint the client
    /// is configured with.
    host: &'static str,
    region: &'static str,
    stage: &'static str,
    /// `IdentifierParts$Method` and `$Path`: which endpoint of the stage the page documents.
    /// Neither takes part in the address the page publishes.
    method: &'static str,
    path: &'static str,
    announced: Announced,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: a client configured with the emulator's loopback \
                   endpoint",
            host: "127.0.0.1:39033",
            region: "us-west-2",
            stage: "prod",
            method: "POST",
            path: "/orders",
            announced: Announced::EmulatorEndpoint,
        },
        Row {
            case: "the same endpoint named by its loopback NAME on another port",
            host: "localhost:9297",
            region: "ap-south-1",
            stage: "beta-2",
            method: "GET",
            path: "/accounts/{accountId}/orders",
            announced: Announced::EmulatorEndpoint,
        },
        Row {
            case: "a client that resolves AWS's own hosts: the documented execute-api host",
            host: "apigateway.eu-north-1.amazonaws.com",
            region: "eu-north-1",
            stage: "stage_3",
            method: "DELETE",
            path: "/",
            announced: Announced::ExecuteApiHost,
        },
    ]
}

/// The address the row's page must publish for the stage of `rest_api_id`, written out here
/// rather than taken from the service, so the table asserts the rule and not the
/// implementation.
fn expected_endpoint(row: &Row, rest_api_id: &str) -> String {
    match row.announced {
        Announced::EmulatorEndpoint => format!(
            "https://{}/_aws/execute-api/{rest_api_id}/{}",
            row.host, row.stage
        ),
        Announced::ExecuteApiHost => format!(
            "https://{rest_api_id}.execute-api.{}.amazonaws.com/{}",
            row.region, row.stage
        ),
    }
}

/// The published endpoint split into the authority a client dials and the path it carries —
/// empty for an endpoint that is a bare host.
///
/// `pub(crate)` because it is the one split every suite that DIALS an address this service
/// published needs, the exported definition's one Server Object among them
/// (`crate::api_export_published_server`).
pub(crate) fn dialed_parts(endpoint: &str) -> (String, String) {
    let without_scheme = endpoint
        .split_once("://")
        .map_or(endpoint, |(_scheme, rest)| rest);
    match without_scheme.find('/') {
        Some(at) => (
            without_scheme[..at].to_string(),
            without_scheme[at..].to_string(),
        ),
        None => (without_scheme.to_string(), String::new()),
    }
}

/// Send one control-plane call at the row's endpoint and assert it was accepted.
async fn accepted(
    emulator: &MultiServiceRouter,
    row: &Row,
    what: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Value {
    let (status, answered) = call_at_host(emulator, row.host, row.region, method, uri, body).await;
    assert!(
        (200..300).contains(&status),
        "case {:?}: {what} must be accepted, got {status} {answered}",
        row.case
    );
    answered
}

/// Create one REST API through the service that owns them, give its root resource the row's
/// documented method fronted by a `MOCK` integration that answers [`SERVED_BODY`], deploy the
/// row's stage, and answer the `RestApiId`.
async fn rest_api_with_served_stage(
    emulator: &MultiServiceRouter,
    row: &Row,
    name: &str,
) -> String {
    let created = accepted(
        emulator,
        row,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": name })),
    )
    .await;
    let rest_api_id = as_str(&created, "id");
    let root = as_str(&created, "rootResourceId");
    let method = row.method;

    accepted(
        emulator,
        row,
        "PutMethod",
        "PUT",
        &format!("/restapis/{rest_api_id}/resources/{root}/methods/{method}"),
        Some(json!({ "authorizationType": "NONE" })),
    )
    .await;
    accepted(
        emulator,
        row,
        "PutIntegration",
        "PUT",
        &format!("/restapis/{rest_api_id}/resources/{root}/methods/{method}/integration"),
        Some(json!({
            "type": "MOCK",
            "requestTemplates": { JSON: "{\"statusCode\": 200}" }
        })),
    )
    .await;
    accepted(
        emulator,
        row,
        "PutMethodResponse",
        "PUT",
        &format!("/restapis/{rest_api_id}/resources/{root}/methods/{method}/responses/200"),
        Some(json!({})),
    )
    .await;
    accepted(
        emulator,
        row,
        "PutIntegrationResponse",
        "PUT",
        &format!(
            "/restapis/{rest_api_id}/resources/{root}/methods/{method}/integration/responses/200"
        ),
        Some(json!({ "responseTemplates": { JSON: SERVED_BODY } })),
    )
    .await;
    accepted(
        emulator,
        row,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{rest_api_id}/deployments"),
        Some(json!({ "stageName": row.stage })),
    )
    .await;

    rest_api_id
}

/// The `displayContent.endpoint` a page response reports.
fn published(case: &str, document: &Value) -> String {
    document
        .get("displayContent")
        .and_then(|content| content.get("endpoint"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: a page must report a displayContent.endpoint: {document}")
        })
        .to_string()
}

/// Assert `reporter` published `expected`.
fn assert_published(case: &str, reporter: &str, document: &Value, expected: &str) {
    let reported = published(case, document);
    assert_eq!(
        reported, expected,
        "case {case:?}: {reporter} must publish the address this emulator serves the \
         documented stage at ({expected:?}), got {reported:?}"
    );
}

/// Dial `endpoint` and assert the documented stage's integration answered there.
async fn assert_dialable(case: &str, emulator: &MultiServiceRouter, row: &Row, endpoint: &str) {
    let (authority, path) = dialed_parts(endpoint);
    let at = if path.is_empty() { "/".to_string() } else { path };
    let answer = send_with_body(
        emulator,
        &authority,
        row.method,
        &at,
        &[("content-type", JSON)],
        b"{}".to_vec(),
    )
    .await;
    assert_ne!(
        answer.error_type.as_deref(),
        Some(UNKNOWN_SERVICE),
        "case {case:?}: the published endpoint must be an address this emulator serves, but \
         {authority}{at} is claimed by no service: {}",
        answer.summary()
    );
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: the published endpoint {authority}{at} must reach the documented \
         stage's integration, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.text, SERVED_BODY,
        "case {case:?}: the published endpoint must reach THAT stage's integration, got {}",
        answer.summary()
    );
}

/// Create one portal product at the row's endpoint and answer its id.
async fn product_at(emulator: &MultiServiceRouter, row: &Row, name: &str) -> String {
    let created = accepted(
        emulator,
        row,
        "CreatePortalProduct",
        "POST",
        "/v2/portalproducts",
        Some(json!({ "displayName": name })),
    )
    .await;
    as_str(&created, "portalProductId")
}

/// Every operation that reports a product REST endpoint page publishes the one address this
/// emulator serves the documented REST stage at, and a request to that address reaches the
/// stage's integration.
#[tokio::test]
async fn a_pages_published_endpoint_is_the_address_the_documented_stage_is_served_on() {
    for (index, row) in rows().into_iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let rest_api_id =
            rest_api_with_served_stage(&emulator, &row, &format!("published-endpoint-{index}"))
                .await;
        let expected = expected_endpoint(&row, &rest_api_id);
        let product = product_at(&emulator, &row, &format!("Published {index}")).await;
        let collection = pages_of(&product);

        let created = accepted(
            &emulator,
            &row,
            "CreateProductRestEndpointPage",
            "POST",
            &collection,
            Some(json!({
                "restEndpointIdentifier": {
                    "identifierParts": {
                        "restApiId": rest_api_id,
                        "stage": row.stage,
                        "method": row.method,
                        "path": row.path
                    }
                }
            })),
        )
        .await;
        assert_published(case, "CreateProductRestEndpointPage", &created, &expected);
        let page = format!("{collection}/{}", as_str(&created, "productRestEndpointPageId"));

        let read = accepted(
            &emulator,
            &row,
            "GetProductRestEndpointPage",
            "GET",
            &page,
            None,
        )
        .await;
        assert_published(case, "GetProductRestEndpointPage", &read, &expected);

        // A patch that rewrites the page's content re-resolves the derived endpoint, so the
        // update cannot leave a consumer holding an address other than the one served.
        let patched = accepted(
            &emulator,
            &row,
            "UpdateProductRestEndpointPage",
            "PATCH",
            &page,
            Some(json!({ "displayContent": { "none": {} } })),
        )
        .await;
        assert_published(case, "UpdateProductRestEndpointPage", &patched, &expected);

        // The address every one of them published is dialable, and reaches the stage.
        assert_dialable(case, &emulator, &row, &expected).await;

        // An endpoint the caller OVERRODE is the caller's own value, reported as given — and
        // a later patch that names no override returns to the served address.
        let overridden = "https://docs.example.com/orders";
        let overriding = accepted(
            &emulator,
            &row,
            "UpdateProductRestEndpointPage with an overriding endpoint",
            "PATCH",
            &page,
            Some(json!({
                "displayContent": { "overrides": { "endpoint": overridden } }
            })),
        )
        .await;
        assert_published(case, "an overriding update", &overriding, overridden);
        let back = accepted(
            &emulator,
            &row,
            "UpdateProductRestEndpointPage dropping the override",
            "PATCH",
            &page,
            Some(json!({ "displayContent": { "none": {} } })),
        )
        .await;
        assert_published(case, "the update dropping the override", &back, &expected);
    }
}
