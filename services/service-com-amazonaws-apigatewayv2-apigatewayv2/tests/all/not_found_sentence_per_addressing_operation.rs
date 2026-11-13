// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the SENTENCE an ApiGatewayV2 `NotFoundException` carries,
//! whichever operation addressed the resource that resolved to nothing.
//!
//! A refusal's message is part of the contract a client keys on: it is the only place the
//! service says WHICH resource of the request could not be resolved, so a client that
//! surfaces or parses it depends on the sentence as much as on the 404. The rules these
//! cases assert:
//!
//! * **A 404 carries the sentence of the KIND whose record the operation needed**, the
//!   same one for every operation that addresses that kind — its reader, its writer, its
//!   delete and a create under it — with the sentence's variable parts taken from the
//!   request and from the account the lookup was made in. Three forms appear: one BUILT
//!   from the identifiers the request addressed the resource with (an integration
//!   response names its own id and the integration it hangs off), one that is FIXED TEXT
//!   for its whole class (a route response, a product page, a product REST endpoint
//!   page), and one ACCOUNT-QUALIFIED (a portal product, which is addressed within an
//!   account rather than within another resource).
//! * **An operation addressed at a PRODUCT, or at one of its collections, resolves the
//!   PRODUCT**: reading it, patching it, reading, replacing or clearing its sharing
//!   policy, creating a page under it and deleting a page under it all name the product
//!   that resolved to nothing — even when the page id they carry is one this account did
//!   mint, under another product. The readers and writers of ONE page name that page's
//!   kind instead.
//! * **A refused lookup writes nothing**: after every row the fixture's resources still
//!   read back as they were created.
//!
//! The rows span what the rules generalise over: the kind of resource, the operation that
//! addresses it (read, patch, create-under-parent, delete-under-parent), and the form of
//! the identifier that resolves to nothing (a short one, a single character, the longest
//! the model's id members admit, a mixed-case one, and — for the delete legs — an id this
//! account minted under a different parent). The recorded calls that filed the issue, one
//! each of `GetIntegrationResponse`, `GetPortalProduct`, `UpdatePortalProduct`,
//! `CreateProductPage`, `GetProductPage`, `DeleteProductRestEndpointPage`,
//! `GetRouteResponse` and `UpdateRouteResponse`, are one row each.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{
    as_str, call, emulator_with_rest_apis, error_code, rest_endpoint,
};

/// The portal products collection (`/v2/portalproducts`).
const PRODUCTS: &str = "/v2/portalproducts";

/// AWS's sentence for a `RouteResponseId` the addressed route holds no response under:
/// fixed text for the whole class, in the service's own lower-case spelling of the kind,
/// carrying neither the id the request already holds nor the account.
const ROUTE_RESPONSE_NOT_FOUND: &str = "Invalid route response specified";

/// AWS's sentence for a product page that resolves to nothing: the whole sentence is
/// fixed text, with no identifier interpolated.
const PRODUCT_PAGE_NOT_FOUND: &str = "ProductPage not found";

/// AWS's sentence for a product REST endpoint page that resolves to nothing, which takes
/// the same form as the product page's.
const PRODUCT_REST_ENDPOINT_PAGE_NOT_FOUND: &str = "ProductRestEndpointPage not found";

/// AWS's sentence for an `IntegrationResponseId` the addressed integration holds no
/// response under: an integration response is resolvable only THROUGH its integration,
/// so both ids the request addressed it with are named.
fn integration_response_not_found(
    integration_response_id: &str,
    integration_id: &str,
) -> String {
    format!(
        "IntegrationResponse with ID {integration_response_id} is not present in \
         integration with ID {integration_id}"
    )
}

/// AWS's sentence for a `PortalProductId` the account holds no product under: a product
/// is addressed within an ACCOUNT, so the account the lookup was made in is part of the
/// sentence beside the id the request supplied.
fn product_not_found(portal_product_id: &str) -> String {
    format!(
        "Product not found in account {} with id {portal_product_id}",
        engine::emulator_account_id()
    )
}

/// Identifiers nothing minted, spanning the forms a client can send: an eleven-character
/// lowercase-alphanumeric one, a single character, the longest id any id member of this
/// model admits (`__stringMin10Max30PatternAZ09`, len 10..30), and a mixed-case one.
fn unresolvable_ids() -> Vec<String> {
    vec![
        "nosuchid000".to_string(),
        "z".to_string(),
        "9".repeat(30),
        "MixedCase1".to_string(),
    ]
}

/// One operation addressed with an identifier that resolves to nothing, and the sentence
/// its refusal must carry.
struct Row {
    operation: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    expected: String,
}

/// Send one request that must be accepted with `status`, and answer its body.
async fn accepted(
    emulator: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
    status: u16,
) -> Value {
    let (answered, parsed) = call(emulator, method, uri, body).await;
    assert_eq!(
        answered, status,
        "case {case:?}: expected {status} from {method} {uri}, got {answered} {parsed}"
    );
    parsed
}

/// Assert one row end to end: the status, the error code, and the message. Answers the
/// message the service returned, so a suite can assert the sentence's FORM on what came
/// back over the wire rather than on what it expected.
async fn assert_not_found(
    emulator: &MultiServiceRouter,
    case: &str,
    row: &Row,
) -> String {
    let (status, body) = call(emulator, row.method, &row.uri, row.body.clone()).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404 from {} {}, got {status} {body}",
        row.operation, row.uri
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "case {case:?}: expected NotFoundException from {}, got {body}",
        row.operation
    );
    let message = as_str(&body, "message");
    assert_eq!(
        message, row.expected,
        "case {case:?}: expected {} to be refused with {:?}, got {body}",
        row.operation, row.expected
    );
    message
}

/// The resources every row of this suite is addressed against.
struct Fixture {
    api_id: String,
    route_id: String,
    integration_id: String,
    product_id: String,
    /// A product page of [`Fixture::product_id`], so a delete addressed at an
    /// unresolvable PRODUCT can still carry a page id this account minted.
    product_page_id: String,
    /// A product REST endpoint page of [`Fixture::product_id`], for the same reason.
    rest_endpoint_page_id: String,
}

/// One WebSocket api carrying an integration and a route (the two kinds that own
/// responses), and one portal product carrying a page of each kind.
async fn fixture(emulator: &MultiServiceRouter, name: &str) -> Fixture {
    let api = accepted(
        emulator,
        "the api",
        "POST",
        "/v2/apis",
        Some(json!({
            "name": name,
            "protocolType": "WEBSOCKET",
            // "This property is required for WebSocket APIs"
            // (`CreateApi$RouteSelectionExpression`), and route responses are a
            // WebSocket-only resource.
            "routeSelectionExpression": "$request.body.action"
        })),
        201,
    )
    .await;
    let api_id = as_str(&api, "apiId");

    let integration = accepted(
        emulator,
        "the integration",
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        // "MOCK: for integrating the route or method request with API Gateway as a
        // 'loopback' endpoint without invoking any backend"
        // (`CreateIntegration$IntegrationType`), so it names no endpoint.
        Some(json!({ "integrationType": "MOCK" })),
        201,
    )
    .await;
    let integration_id = as_str(&integration, "integrationId");

    let route = accepted(
        emulator,
        "the route",
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": "$default",
            "target": format!("integrations/{integration_id}")
        })),
        201,
    )
    .await;
    let route_id = as_str(&route, "routeId");

    let product = accepted(
        emulator,
        "the product",
        "POST",
        PRODUCTS,
        Some(json!({ "displayName": name })),
        201,
    )
    .await;
    let product_id = as_str(&product, "portalProductId");

    let page = accepted(
        emulator,
        "the product page",
        "POST",
        &format!("{PRODUCTS}/{product_id}/productpages"),
        Some(json!({
            "displayContent": { "title": "Getting started", "body": "A body." }
        })),
        201,
    )
    .await;
    let product_page_id = as_str(&page, "productPageId");

    let rest_api = rest_endpoint(emulator, &format!("{name}-rest"), "prod").await;
    let endpoint_page = accepted(
        emulator,
        "the product REST endpoint page",
        "POST",
        &format!("{PRODUCTS}/{product_id}/productrestendpointpages"),
        // `RestEndpointIdentifier$IdentifierParts`: the REST API, stage, method and path
        // of the endpoint the page documents — one the account HOLDS, created through the
        // service that owns REST APIs.
        Some(json!({
            "restEndpointIdentifier": {
                "identifierParts": {
                    "restApiId": rest_api,
                    "stage": "prod",
                    "method": "GET",
                    "path": "/orders"
                }
            }
        })),
        201,
    )
    .await;
    let rest_endpoint_page_id = as_str(&endpoint_page, "productRestEndpointPageId");

    Fixture {
        api_id,
        route_id,
        integration_id,
        product_id,
        product_page_id,
        rest_endpoint_page_id,
    }
}

/// Every operation that addresses one RESPONSE of an api, addressed with `id`.
fn response_rows(fixture: &Fixture, id: &str) -> Vec<Row> {
    let integration_responses = format!(
        "/v2/apis/{}/integrations/{}/integrationresponses/{id}",
        fixture.api_id, fixture.integration_id
    );
    let route_responses = format!(
        "/v2/apis/{}/routes/{}/routeresponses/{id}",
        fixture.api_id, fixture.route_id
    );
    let integration_response_sentence =
        integration_response_not_found(id, &fixture.integration_id);

    vec![
        Row {
            operation: "GetIntegrationResponse",
            method: "GET",
            uri: integration_responses.clone(),
            body: None,
            expected: integration_response_sentence.clone(),
        },
        Row {
            operation: "DeleteIntegrationResponse",
            method: "DELETE",
            uri: integration_responses,
            body: None,
            expected: integration_response_sentence,
        },
        Row {
            operation: "GetRouteResponse",
            method: "GET",
            uri: route_responses.clone(),
            body: None,
            expected: ROUTE_RESPONSE_NOT_FOUND.to_string(),
        },
        Row {
            operation: "UpdateRouteResponse",
            method: "PATCH",
            uri: route_responses.clone(),
            body: Some(json!({ "routeResponseKey": "$default" })),
            expected: ROUTE_RESPONSE_NOT_FOUND.to_string(),
        },
        Row {
            operation: "DeleteRouteResponse",
            method: "DELETE",
            uri: route_responses,
            body: None,
            expected: ROUTE_RESPONSE_NOT_FOUND.to_string(),
        },
    ]
}

/// Every operation addressed at a PRODUCT or at one of its collections, with `id` in
/// place of the `PortalProductId`.
fn product_rows(fixture: &Fixture, id: &str) -> Vec<Row> {
    let product = format!("{PRODUCTS}/{id}");
    let expected = product_not_found(id);

    vec![
        Row {
            operation: "GetPortalProduct",
            method: "GET",
            uri: product.clone(),
            body: None,
            expected: expected.clone(),
        },
        Row {
            operation: "UpdatePortalProduct",
            method: "PATCH",
            uri: product.clone(),
            body: Some(json!({ "displayName": "Renamed" })),
            expected: expected.clone(),
        },
        Row {
            operation: "GetPortalProductSharingPolicy",
            method: "GET",
            uri: format!("{product}/sharingpolicy"),
            body: None,
            expected: expected.clone(),
        },
        Row {
            operation: "PutPortalProductSharingPolicy",
            method: "PUT",
            uri: format!("{product}/sharingpolicy"),
            body: Some(json!({
                "policyDocument": "{\"Version\":\"2012-10-17\",\"Statement\":[]}"
            })),
            expected: expected.clone(),
        },
        Row {
            operation: "DeletePortalProductSharingPolicy",
            method: "DELETE",
            uri: format!("{product}/sharingpolicy"),
            body: None,
            expected: expected.clone(),
        },
        Row {
            operation: "CreateProductPage",
            method: "POST",
            uri: format!("{product}/productpages"),
            body: Some(json!({
                "displayContent": { "title": "New", "body": "A body." }
            })),
            expected: expected.clone(),
        },
        Row {
            operation: "CreateProductRestEndpointPage",
            method: "POST",
            uri: format!("{product}/productrestendpointpages"),
            body: Some(json!({
                "restEndpointIdentifier": {
                    "identifierParts": {
                        "restApiId": "a1b2c3d4e5",
                        "stage": "prod",
                        "method": "GET",
                        "path": "/orders"
                    }
                }
            })),
            expected: expected.clone(),
        },
        // A delete under an unresolvable product names the PRODUCT, not the child — and
        // the page ids these two rows carry are ones this account DID mint, under the
        // fixture's product, so what the refusal reports is the parent the request
        // addressed and not merely "nothing of that id exists".
        Row {
            operation: "DeleteProductPage",
            method: "DELETE",
            uri: format!("{product}/productpages/{}", fixture.product_page_id),
            body: None,
            expected: expected.clone(),
        },
        Row {
            operation: "DeleteProductRestEndpointPage",
            method: "DELETE",
            uri: format!(
                "{product}/productrestendpointpages/{}",
                fixture.rest_endpoint_page_id
            ),
            body: None,
            expected,
        },
    ]
}

/// Every operation addressed at ONE page of the product the account holds, with `id` in
/// place of the page's own identifier.
fn page_rows(fixture: &Fixture, id: &str) -> Vec<Row> {
    let product_page = format!("{PRODUCTS}/{}/productpages/{id}", fixture.product_id);
    let endpoint_page = format!(
        "{PRODUCTS}/{}/productrestendpointpages/{id}",
        fixture.product_id
    );

    vec![
        Row {
            operation: "GetProductPage",
            method: "GET",
            uri: product_page.clone(),
            body: None,
            expected: PRODUCT_PAGE_NOT_FOUND.to_string(),
        },
        Row {
            operation: "UpdateProductPage",
            method: "PATCH",
            uri: product_page,
            body: Some(json!({
                "displayContent": { "title": "Patched", "body": "A body." }
            })),
            expected: PRODUCT_PAGE_NOT_FOUND.to_string(),
        },
        Row {
            operation: "GetProductRestEndpointPage",
            method: "GET",
            uri: endpoint_page.clone(),
            body: None,
            expected: PRODUCT_REST_ENDPOINT_PAGE_NOT_FOUND.to_string(),
        },
        Row {
            operation: "UpdateProductRestEndpointPage",
            method: "PATCH",
            uri: endpoint_page,
            body: Some(json!({ "tryItState": "ENABLED" })),
            expected: PRODUCT_REST_ENDPOINT_PAGE_NOT_FOUND.to_string(),
        },
    ]
}

/// Read back everything the fixture created, so a table of refusals is shown to have
/// written nothing: the api's integration and route, the product, and its two pages.
async fn assert_fixture_intact(emulator: &MultiServiceRouter, case: &str, fixture: &Fixture) {
    let product = accepted(
        emulator,
        case,
        "GET",
        &format!("{PRODUCTS}/{}", fixture.product_id),
        None,
        200,
    )
    .await;
    assert_eq!(
        as_str(&product, "portalProductId"),
        fixture.product_id,
        "case {case:?}: expected GetPortalProduct to report the fixture's product, \
         got {product}"
    );

    let page = accepted(
        emulator,
        case,
        "GET",
        &format!(
            "{PRODUCTS}/{}/productpages/{}",
            fixture.product_id, fixture.product_page_id
        ),
        None,
        200,
    )
    .await;
    assert_eq!(
        as_str(&page, "productPageId"),
        fixture.product_page_id,
        "case {case:?}: expected GetProductPage to report the fixture's page, got {page}"
    );

    let endpoint_page = accepted(
        emulator,
        case,
        "GET",
        &format!(
            "{PRODUCTS}/{}/productrestendpointpages/{}",
            fixture.product_id, fixture.rest_endpoint_page_id
        ),
        None,
        200,
    )
    .await;
    assert_eq!(
        as_str(&endpoint_page, "productRestEndpointPageId"),
        fixture.rest_endpoint_page_id,
        "case {case:?}: expected GetProductRestEndpointPage to report the fixture's \
         page, got {endpoint_page}"
    );

    let integration = accepted(
        emulator,
        case,
        "GET",
        &format!(
            "/v2/apis/{}/integrations/{}",
            fixture.api_id, fixture.integration_id
        ),
        None,
        200,
    )
    .await;
    assert_eq!(
        as_str(&integration, "integrationId"),
        fixture.integration_id,
        "case {case:?}: expected GetIntegration to report the fixture's integration, \
         got {integration}"
    );

    let route = accepted(
        emulator,
        case,
        "GET",
        &format!("/v2/apis/{}/routes/{}", fixture.api_id, fixture.route_id),
        None,
        200,
    )
    .await;
    assert_eq!(
        as_str(&route, "routeId"),
        fixture.route_id,
        "case {case:?}: expected GetRoute to report the fixture's route, got {route}"
    );
}

#[tokio::test]
async fn every_operation_addressing_one_response_of_an_api_answers_that_kinds_sentence() {
    let emulator = emulator_with_rest_apis();
    let fixture = fixture(&emulator, "response-sentence").await;

    for id in unresolvable_ids() {
        for row in response_rows(&fixture, &id) {
            let case = format!("{} addressed as {id:?}", row.operation);
            assert_not_found(&emulator, &case, &row).await;
        }
    }

    assert_fixture_intact(&emulator, "after the response refusals", &fixture).await;
}

#[tokio::test]
async fn every_operation_addressed_at_a_portal_product_names_the_product_and_the_account() {
    let emulator = emulator_with_rest_apis();
    let fixture = fixture(&emulator, "product-sentence").await;
    let account = engine::emulator_account_id();

    for id in unresolvable_ids() {
        for row in product_rows(&fixture, &id) {
            let case = format!("{} addressed at product {id:?}", row.operation);
            let message = assert_not_found(&emulator, &case, &row).await;
            // What the service ANSWERED reports the account the lookup was made in and
            // the identifier the request supplied: an account-level resource is reported
            // account-qualified.
            assert!(
                message.contains(account) && message.contains(&id),
                "case {case:?}: expected the answered sentence to report the account \
                 {account:?} and the identifier, got {message:?}"
            );
        }
    }

    assert_fixture_intact(&emulator, "after the product refusals", &fixture).await;
}

#[tokio::test]
async fn every_operation_addressed_at_one_page_answers_that_pages_fixed_sentence() {
    let emulator = emulator_with_rest_apis();
    let fixture = fixture(&emulator, "page-sentence").await;
    let account = engine::emulator_account_id();

    for id in unresolvable_ids() {
        for row in page_rows(&fixture, &id) {
            let case = format!("{} addressed at page {id:?}", row.operation);
            let message = assert_not_found(&emulator, &case, &row).await;
            // A page's sentence is fixed text for its whole class, so what the service
            // ANSWERED carries neither the identifier the request already holds nor the
            // account.
            assert!(
                !message.contains(&id) && !message.contains(account),
                "case {case:?}: expected the answered sentence to carry no identifier \
                 and no account, got {message:?}"
            );
        }
    }

    assert_fixture_intact(&emulator, "after the page refusals", &fixture).await;
}
