// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how this service FRAMES an answer that carries no content.
//!
//! The rule these cases assert: **every answer of this service that carries no content —
//! the `204 No Content` its empty-output (`smithy.api#Unit`) operations are modelled to
//! give — carries no body at all and STATES the length it has, `content-length: 0`, rather
//! than leaving the framing header off; an answer that DOES carry content keeps its body
//! and is not reframed.** The modelled statuses come from the model itself
//! (`smithy_query(query="digest")`: `DeleteApi`, `DeleteRoute`, `DeleteStage`,
//! `DeleteAccessLogSettings`, `DeleteCorsConfiguration`, `DeleteProductPage`,
//! `DeleteProductRestEndpointPage`, `DeletePortalProductSharingPolicy` are all
//! `code: 204` over `Unit`), and what a client reads the message end from is the stated
//! length.
//!
//! The rows span what the rule generalises over: the resource family (an api, a child of an
//! api, a settings member of a stage, a portal product's pages, a policy member of a
//! product), whether the delete removed something or asked for a state the store was
//! already in, and — in the second test — a 2xx and a 4xx that carry a body.
//!
//! The recorded step that filed the issue — one product page created under one portal
//! product and then deleted — is ONE row of the first table.
//!
//! NOTE: what this suite pins is the framing the SERVICE states, which the in-process front
//! door (`MultiServiceRouter::dispatch`, the inter-service callback path and the SDK
//! connector) delivers unchanged. A client reached over HTTP/1.1 does not see the header:
//! the HTTP writer the vendored transport uses removes `content-length` from a `204`, which
//! is below anything this crate decides.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{
    as_str, call, call_reading_headers, emulator, emulator_with_rest_apis, error_code,
    rest_endpoint,
};
use crate::product_page_lifecycle::pages_of as product_pages_of;
use crate::product_rest_endpoint_page_lifecycle::{
    pages_of as rest_endpoint_pages_of, product, PRODUCTS,
};

/// The length an answer that carries no content states. Reused by `mutating_success_status`,
/// so both suites judge the framing by one value.
pub(crate) const NO_CONTENT_LENGTH: &str = "0";

/// The status such an answer carries.
const NO_CONTENT_STATUS: u16 = 204;

/// The `content-length` a response states, if any. The ONE reading of that header in this
/// test target, shared with `mutating_success_status`.
pub(crate) fn content_length(headers: &http::HeaderMap) -> Option<String> {
    headers
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// `(status, headers, parsed body)` of a DELETE of `uri`.
async fn delete(emulator: &MultiServiceRouter, uri: &str) -> (u16, http::HeaderMap, Value) {
    call_reading_headers(emulator, "DELETE", uri, None).await
}

/// Assert `case`'s answer is an empty 204 that states the length it has.
fn assert_empty_answer(case: &str, status: u16, headers: &http::HeaderMap, body: &Value) {
    assert_eq!(
        status, NO_CONTENT_STATUS,
        "case {case:?}: expected {NO_CONTENT_STATUS}, got {status} {body}"
    );
    assert_eq!(
        *body,
        Value::Null,
        "case {case:?}: expected an answer carrying no body, got {body}"
    );
    assert_eq!(
        content_length(headers).as_deref(),
        Some(NO_CONTENT_LENGTH),
        "case {case:?}: expected the answer to state content-length {NO_CONTENT_LENGTH}, got \
         {:?}",
        content_length(headers)
    );
}

/// Create one HTTP api carrying a CORS configuration and answer its `ApiId`.
async fn api_with_cors(emulator: &MultiServiceRouter) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": "framing-api",
            "protocolType": "HTTP",
            "corsConfiguration": { "allowOrigins": ["https://example.com"] }
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

#[tokio::test]
async fn every_answer_that_carries_no_content_states_content_length_zero() {
    // The REST endpoint a product REST endpoint page documents is created through the
    // service that OWNS REST APIs, so this emulator holds both.
    let emulator = emulator_with_rest_apis();

    // The api family: an api, a route of it, a stage of it, that stage's access log
    // settings, and the api's CORS configuration.
    let api = api_with_cors(&emulator).await;
    let (status, created) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api}/routes"),
        Some(json!({ "routeKey": "GET /items" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateRoute, got {status} {created}"
    );
    let route = as_str(&created, "routeId");
    let (status, created) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api}/stages"),
        Some(json!({
            "stageName": "prod",
            "accessLogSettings": {
                "destinationArn": "arn:aws:logs:eu-west-1:000000000000:log-group:framing",
                "format": "$context.requestId"
            }
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateStage, got {status} {created}"
    );
    let stage = as_str(&created, "stageName");

    // The portal-product family: a product, a product page, a REST endpoint page and a
    // sharing policy.
    let portal_product = product(&emulator, "Framing product").await;
    let rest_api = rest_endpoint(&emulator, "framing-rest", "prod").await;
    let (status, created) = call(
        &emulator,
        "POST",
        &product_pages_of(&portal_product),
        Some(json!({ "displayContent": { "title": "Overview", "body": "A body" } })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateProductPage, got {status} {created}"
    );
    let product_page = as_str(&created, "productPageId");
    let (status, created) = call(
        &emulator,
        "POST",
        &rest_endpoint_pages_of(&portal_product),
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
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateProductRestEndpointPage, got {status} {created}"
    );
    let rest_endpoint_page = as_str(&created, "productRestEndpointPageId");
    let (status, written) = call(
        &emulator,
        "PUT",
        &format!("{PRODUCTS}/{portal_product}/sharingpolicy"),
        Some(json!({
            "policyDocument": "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\
                \"Allow\",\"Principal\":{\"AWS\":\"*\"},\"Action\":\"apigateway:GET\"}]}"
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from PutPortalProductSharingPolicy, got {status} {written}"
    );

    // Deleted parent last, so every row addresses a resource that is still there.
    let rows: Vec<(&str, String)> = vec![
        (
            "DeleteCorsConfiguration, a member of an api",
            format!("/v2/apis/{api}/cors"),
        ),
        (
            "DeleteAccessLogSettings, a member of a stage",
            format!("/v2/apis/{api}/stages/{stage}/accesslogsettings"),
        ),
        (
            "DeleteRoute, a child of an api",
            format!("/v2/apis/{api}/routes/{route}"),
        ),
        (
            "DeleteStage, a child of an api addressed by name",
            format!("/v2/apis/{api}/stages/{stage}"),
        ),
        (
            "DeleteProductPage, a child of a portal product",
            format!("{}/{product_page}", product_pages_of(&portal_product)),
        ),
        (
            "DeleteProductPage again, a page that is already gone",
            format!("{}/{product_page}", product_pages_of(&portal_product)),
        ),
        (
            "DeleteProductRestEndpointPage, a child of a portal product",
            format!(
                "{}/{rest_endpoint_page}",
                rest_endpoint_pages_of(&portal_product)
            ),
        ),
        (
            "DeletePortalProductSharingPolicy, a member of a portal product",
            format!("{PRODUCTS}/{portal_product}/sharingpolicy"),
        ),
        (
            "DeletePortalProductSharingPolicy again, a policy that is already clear",
            format!("{PRODUCTS}/{portal_product}/sharingpolicy"),
        ),
        ("DeleteApi, a top-level resource", format!("/v2/apis/{api}")),
    ];

    for (case, uri) in rows {
        let (status, headers, body) = delete(&emulator, &uri).await;
        assert_empty_answer(case, status, &headers, &body);
    }
}

/// An answer that CARRIES content is not reframed: it keeps its body, and any length it
/// states is that body's, never the empty answer's zero.
#[tokio::test]
async fn an_answer_that_carries_content_keeps_its_body_and_its_own_length() {
    let emulator = emulator();

    let (status, headers, created) = call_reading_headers(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "bodied-api", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"CreateApi\": expected 201, got {status} {created}"
    );
    assert_eq!(
        as_str(&created, "name"),
        "bodied-api",
        "case \"CreateApi\": expected the created api in the body, got {created}"
    );
    assert_ne!(
        content_length(&headers).as_deref(),
        Some(NO_CONTENT_LENGTH),
        "case \"CreateApi\": a response carrying a body does not state content-length \
         {NO_CONTENT_LENGTH}, got {created}"
    );

    // A refusal carries an error envelope, which is content like any other.
    let (status, headers, refused) = call_reading_headers(
        &emulator,
        "DELETE",
        "/v2/apis/nosuchapi00/routes/nosuchroute",
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"DeleteRoute of an unresolved api\": expected 404, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "NotFoundException",
        "case \"DeleteRoute of an unresolved api\": expected NotFoundException, got {refused}"
    );
    assert_ne!(
        content_length(&headers).as_deref(),
        Some(NO_CONTENT_LENGTH),
        "case \"DeleteRoute of an unresolved api\": a refusal carrying an envelope does not \
         state content-length {NO_CONTENT_LENGTH}, got {refused}"
    );
}
