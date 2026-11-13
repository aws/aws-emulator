// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `ProductRestEndpointPage` lifecycle.
//!
//! The rules these cases assert:
//!
//! * **`CreateProductRestEndpointPage` persists a page UNDER the portal product the URI
//!   names and answers 201** with the `ProductRestEndpointPageId` it minted (ten
//!   lowercase alphanumerics, which satisfies the modelled
//!   `__stringMin10Max30PatternAZ09`), the ARN naming that product and the Region the
//!   request addressed, a `LastModified` reading the write instant, the REST endpoint the
//!   request identified, and the `TryItState` it asked for.
//!   `GetProductRestEndpointPage` reports that same record, member for member.
//! * **A page's display content is the default API Gateway derives from the identified
//!   REST endpoint, with each member the request overrode replacing its default**: the
//!   `Endpoint` defaults to the invoke URL of that endpoint's STAGE
//!   (`https://{restApiId}.execute-api.{region}.amazonaws.com/{stage}`, with neither the
//!   method nor the endpoint's own path appended), while `Body` and `OperationName` are
//!   reported only where the request supplied them, since their defaults live in the REST
//!   API this emulator does not hold.
//! * **A request that identifies no REST endpoint, or whose `DisplayContent` names
//!   neither or both of its alternatives, is refused 400** and persists nothing.
//! * **A page is addressed under its own product**: a product nothing minted is 404, and
//!   so is a page id nothing minted, a page read under another product, and a read naming
//!   another account as the resource owner.
//! * **The page's `Status` is owed by the clock**: a created page is `IN_PROGRESS` while
//!   its content is being generated and reports what that generation settled in once the
//!   span has passed; reading it changes nothing a later caller sees. WHICH terminal
//!   status it settles in follows from the REST endpoint it documents, which is
//!   `product_rest_endpoint_page_generation`'s subject.
//! * **`RawDisplayContent` is reported only for a request that asked for it**, and an
//!   `IncludeRawDisplayContent` that spells no boolean is refused 400.
//! * **One product holds at most `MAX_PRODUCT_REST_ENDPOINT_PAGES_PER_PRODUCT` pages**,
//!   after which a create is refused 429 `TooManyRequestsException`.
//!
//! The rows vary what each rule generalises over: which optional members a request
//! carries, which alternative its display content names, the method, stage and path of
//! the identified endpoint (including a path written without its leading separator and
//! members at their modelled maxima), the try-it state, and the Region the request is
//! signed for. The recorded lifecycle that filed the issue — one create of one page, read
//! back — is ONE row of the first table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{
    as_str, assert_minted_id, call, call_in_region, carries_nothing, emulator_with_rest_apis,
    error_code, instant, read_until, rest_endpoint,
};

/// The Region [`call`] signs for, and the one a derived invoke URL therefore names.
pub(crate) const REGION: &str = "eu-west-1";

/// How far the reported `LastModified` may sit from this test's clock reading, in
/// seconds. The service reports the instant at second granularity, so a whole second of
/// truncation plus scheduling slack is allowed; the point of the assertion is that the
/// member reads the write instant rather than zero or a fixed literal.
const LAST_MODIFIED_SLACK_SECONDS: f64 = 60.0;

/// The portal-products collection URI (`CreatePortalProduct`'s modelled
/// `POST /v2/portalproducts`).
pub(crate) const PRODUCTS: &str = "/v2/portalproducts";

/// The pages collection of one product (`POST|GET
/// /v2/portalproducts/{PortalProductId}/productrestendpointpages`).
pub(crate) fn pages_of(portal_product_id: &str) -> String {
    format!("{PRODUCTS}/{portal_product_id}/productrestendpointpages")
}

/// Create one portal product and answer its id — the parent every page in this suite is
/// created under.
pub(crate) async fn product(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        PRODUCTS,
        Some(json!({ "displayName": name })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreatePortalProduct for {name:?}, got {status} {created}"
    );
    as_str(&created, "portalProductId")
}

/// The URL a page reports for the REST endpoint its `identifierParts` name: "the URL to
/// invoke your REST API" (`EndpointDisplayContentResponse$Endpoint`) — the API's default
/// execute-api host and then the STAGE, which is the base a client calls that API on.
/// Neither the endpoint's method (not part of a URL at all) nor its path is appended.
/// Written out here rather than taken from the service, so the table asserts the rule
/// rather than the implementation.
pub(crate) fn invoke_url(rest_api_id: &str, stage: &str, region: &str) -> String {
    format!("https://{rest_api_id}.execute-api.{region}.amazonaws.com/{stage}")
}

/// The REST endpoint one page documents, as the account HOLDS it.
///
/// A write naming an endpoint the account does not have is refused, so every endpoint a case
/// below documents is created through the service that OWNS REST APIs ([`held_endpoint`]):
/// the `restApiId` a request is WRITTEN with is a placeholder naming the API to create, and
/// what the page is created with is the id that service minted.
pub(crate) struct HeldEndpoint {
    /// `RestEndpointIdentifier$IdentifierParts` as the request sends them, naming the minted
    /// REST API.
    pub(crate) parts: Value,
    pub(crate) rest_api_id: String,
    pub(crate) stage: String,
}

impl HeldEndpoint {
    /// The invoke URL a page documenting this endpoint derives in `region`.
    pub(crate) fn invoke_url(&self, region: &str) -> String {
        invoke_url(&self.rest_api_id, &self.stage, region)
    }
}

/// Create the REST endpoint `parts` describe — a REST API named by the placeholder they
/// carry, with the stage they name deployed on it — through the service that OWNS REST APIs,
/// and answer those parts naming the id it minted.
pub(crate) async fn held_endpoint(emulator: &MultiServiceRouter, parts: &Value) -> HeldEndpoint {
    let placeholder = parts
        .get("restApiId")
        .and_then(Value::as_str)
        .expect("every identifier a case holds names a REST API")
        .to_string();
    let stage = parts
        .get("stage")
        .and_then(Value::as_str)
        .expect("every identifier a case holds names a stage")
        .to_string();
    let rest_api_id = rest_endpoint(emulator, &placeholder, &stage).await;
    let mut held = parts.clone();
    held["restApiId"] = json!(rest_api_id);
    HeldEndpoint {
        parts: held,
        rest_api_id,
        stage,
    }
}

/// One accepted-create row: the request's members, and what the stored page must report.
struct Row {
    case: &'static str,
    /// `RestEndpointIdentifier$IdentifierParts` as the request writes them: the REST API
    /// (named by a placeholder — see [`held_endpoint`]), stage, method and path of the
    /// endpoint the page documents.
    parts: Value,
    /// The request's `DisplayContent`, absent for a request that names none.
    display_content: Option<Value>,
    /// The request's `TryItState`, absent for a request that names none.
    try_it_state: Option<&'static str>,
    /// The `displayContent.endpoint` the page must report where the request OVERRODE it;
    /// `None` where the page must report the URL derived from the identified endpoint.
    overridden_endpoint: Option<&'static str>,
    /// The `displayContent.body` the page must report, if any.
    expected_body: Option<String>,
    /// The `displayContent.operationName` the page must report, if any.
    expected_operation_name: Option<String>,
    /// The `tryItState` the page must report.
    expected_try_it_state: &'static str,
}

/// A `stage` at the modelled maximum (`IdentifierParts$Stage`, len 1..128).
///
/// There is no helper for a `RestApiId` at ITS modelled maximum: the id a page documents is
/// one the service that OWNS REST APIs minted ([`held_endpoint`]), so its length is that
/// service's to choose, not a case's.
pub(crate) fn max_stage() -> String {
    "s".repeat(128)
}

/// A `body` at the modelled maximum (`DisplayContentOverrides$Body`, len 1..32768).
pub(crate) fn max_body() -> String {
    "b".repeat(32768)
}

fn accepted_rows() -> Vec<Row> {
    vec![
        Row {
            case: "no display content and no try-it state: every default",
            parts: json!({
                "restApiId": "a1b2c3d4e5",
                "stage": "prod",
                "method": "GET",
                "path": "/orders"
            }),
            display_content: None,
            try_it_state: None,
            overridden_endpoint: None,
            expected_body: None,
            expected_operation_name: None,
            // The default of a member the request left out.
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "display content naming the `none` alternative: the same defaults",
            parts: json!({
                "restApiId": "zzz999",
                "stage": "test",
                "method": "POST",
                "path": "/orders"
            }),
            display_content: Some(json!({ "none": {} })),
            try_it_state: Some("ENABLED"),
            overridden_endpoint: None,
            expected_body: None,
            expected_operation_name: None,
            expected_try_it_state: "ENABLED",
        },
        Row {
            case: "every override supplied",
            parts: json!({
                "restApiId": "abcde12345",
                "stage": "v1",
                "method": "PUT",
                "path": "/orders/{orderId}"
            }),
            display_content: Some(json!({
                "overrides": {
                    "body": "# Orders\n\nReplace one order.",
                    "endpoint": "https://api.example.com/v1/orders",
                    "operationName": "PutOrder"
                }
            })),
            try_it_state: Some("ENABLED"),
            // An overriding endpoint replaces the derived default wholesale.
            overridden_endpoint: Some("https://api.example.com/v1/orders"),
            expected_body: Some("# Orders\n\nReplace one order.".to_string()),
            expected_operation_name: Some("PutOrder".to_string()),
            expected_try_it_state: "ENABLED",
        },
        Row {
            case: "only the body overridden, so the endpoint keeps its default",
            parts: json!({
                "restApiId": "bodyonly01",
                "stage": "beta",
                "method": "DELETE",
                "path": "/orders/{orderId}"
            }),
            display_content: Some(json!({ "overrides": { "body": "custom documentation" } })),
            try_it_state: Some("DISABLED"),
            overridden_endpoint: None,
            expected_body: Some("custom documentation".to_string()),
            expected_operation_name: None,
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "only the operation name overridden",
            parts: json!({
                "restApiId": "opnameonly",
                "stage": "gamma",
                "method": "PATCH",
                "path": "/orders"
            }),
            display_content: Some(json!({ "overrides": { "operationName": "PatchOrders" } })),
            try_it_state: None,
            overridden_endpoint: None,
            expected_body: None,
            expected_operation_name: Some("PatchOrders".to_string()),
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "an empty overrides structure, which overrides nothing",
            parts: json!({
                "restApiId": "emptyover1",
                "stage": "prod",
                "method": "HEAD",
                "path": "/health"
            }),
            display_content: Some(json!({ "overrides": {} })),
            try_it_state: None,
            overridden_endpoint: None,
            expected_body: None,
            expected_operation_name: None,
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "a path written without its leading separator",
            parts: json!({
                "restApiId": "noslash001",
                "stage": "prod",
                "method": "GET",
                "path": "orders"
            }),
            display_content: None,
            try_it_state: None,
            // The endpoint's path is no part of the URL the page reports, however it is
            // written.
            overridden_endpoint: None,
            expected_body: None,
            expected_operation_name: None,
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "the root path",
            parts: json!({
                "restApiId": "rootpath01",
                "stage": "prod",
                "method": "GET",
                "path": "/"
            }),
            display_content: None,
            try_it_state: None,
            overridden_endpoint: None,
            expected_body: None,
            expected_operation_name: None,
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "a deeply nested path with a greedy variable",
            parts: json!({
                "restApiId": "deeppath01",
                "stage": "prod",
                "method": "GET",
                "path": "/accounts/{accountId}/orders/{orderId}/items/{proxy+}"
            }),
            display_content: None,
            try_it_state: None,
            overridden_endpoint: None,
            expected_body: None,
            expected_operation_name: None,
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "a stage at its modelled maximum",
            parts: json!({
                "restApiId": "maxstage01",
                "stage": max_stage(),
                "method": "GET",
                "path": "/orders"
            }),
            display_content: None,
            try_it_state: None,
            overridden_endpoint: None,
            expected_body: None,
            expected_operation_name: None,
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "an overriding body at the modelled maximum length",
            parts: json!({
                "restApiId": "maxbody001",
                "stage": "prod",
                "method": "GET",
                "path": "/orders"
            }),
            display_content: Some(json!({ "overrides": { "body": max_body() } })),
            try_it_state: None,
            overridden_endpoint: None,
            expected_body: Some(max_body()),
            expected_operation_name: None,
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "content outside ASCII, which round-trips unchanged",
            parts: json!({
                "restApiId": "unicode001",
                "stage": "prod",
                "method": "GET",
                "path": "/注文"
            }),
            display_content: Some(json!({
                "overrides": { "body": "注文 API — ünïcödé 🎉", "operationName": "注文を取得" }
            })),
            try_it_state: None,
            overridden_endpoint: None,
            expected_body: Some("注文 API — ünïcödé 🎉".to_string()),
            expected_operation_name: Some("注文を取得".to_string()),
            expected_try_it_state: "DISABLED",
        },
    ]
}

/// The request one row sends.
fn request_of(row: &Row) -> Value {
    let mut request = json!({ "restEndpointIdentifier": { "identifierParts": row.parts.clone() } });
    if let Some(content) = &row.display_content {
        request["displayContent"] = content.clone();
    }
    if let Some(state) = row.try_it_state {
        request["tryItState"] = json!(state);
    }
    request
}

/// One member of a response's `displayContent`.
pub(crate) fn content_member(document: &Value, member: &str) -> Option<String> {
    document
        .get("displayContent")
        .and_then(|content| content.get(member))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Assert one response reports exactly the page `row` describes, with the endpoint
/// `expected_endpoint` — the derived invoke URL of the endpoint the account holds, or the
/// override the request carried. `reader` names the operation in the failure message.
fn assert_reports_row(row: &Row, reader: &str, document: &Value, expected_endpoint: &str) {
    assert_eq!(
        content_member(document, "endpoint"),
        Some(expected_endpoint.to_string()),
        "case {:?}: expected {reader} to report the endpoint {expected_endpoint:?}, got \
         {document}",
        row.case
    );
    assert_eq!(
        content_member(document, "body"),
        row.expected_body.clone(),
        "case {:?}: expected {reader} to report body {:?}, got {document}",
        row.case,
        row.expected_body
    );
    assert_eq!(
        content_member(document, "operationName"),
        row.expected_operation_name.clone(),
        "case {:?}: expected {reader} to report operationName {:?}, got {document}",
        row.case,
        row.expected_operation_name
    );
    assert_eq!(
        as_str(document, "tryItState"),
        row.expected_try_it_state,
        "case {:?}: expected {reader} to report tryItState {:?}, got {document}",
        row.case,
        row.expected_try_it_state
    );
    assert_eq!(
        document
            .get("restEndpointIdentifier")
            .and_then(|identifier| identifier.get("identifierParts")),
        Some(&row.parts),
        "case {:?}: expected {reader} to report the identified REST endpoint, got {document}",
        row.case
    );
    // `Status` is one of the modelled `AVAILABLE | FAILED | IN_PROGRESS`; which of them a
    // page reports when is asserted by the clock test below. No page this service holds
    // settles `FAILED`: a write naming an endpoint the account does not have is refused
    // (`product_rest_endpoint_page_generation`), so a stored page carries no
    // `StatusException` either.
    let status = as_str(document, "status");
    assert!(
        ["AVAILABLE", "IN_PROGRESS"].contains(&status.as_str()),
        "case {:?}: expected {reader} to report a modelled Status, got {status:?} {document}",
        row.case
    );
    assert!(
        carries_nothing(document, "statusException"),
        "case {:?}: expected {reader} to report no statusException for a page whose endpoint \
         resolved, got {document}",
        row.case
    );
}

#[tokio::test]
async fn create_product_rest_endpoint_page_persists_the_page_and_get_reports_it_back() {
    // A page documents an endpoint the account HOLDS, so the service that owns REST APIs is
    // mounted beside this one and every row's endpoint is created through it.
    let emulator = emulator_with_rest_apis();
    let product_id = product(&emulator, "Orders API").await;
    let collection = pages_of(&product_id);

    let mut minted: Vec<String> = Vec::new();
    for mut row in accepted_rows() {
        let held = held_endpoint(&emulator, &row.parts).await;
        row.parts = held.parts.clone();
        let expected_endpoint = row
            .overridden_endpoint
            .map(str::to_string)
            .unwrap_or_else(|| held.invoke_url(REGION));
        let (status, created) =
            call(&emulator, "POST", &collection, Some(request_of(&row))).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateProductRestEndpointPage, got {status} {created}",
            row.case
        );

        let page_id = as_str(&created, "productRestEndpointPageId");
        assert_minted_id(row.case, "ProductRestEndpointPageId", &page_id);
        minted.push(page_id.clone());

        // The ARN names the page UNDER its product, and is at least the modelled 20
        // characters.
        let arn = as_str(&created, "productRestEndpointPageArn");
        assert_eq!(
            arn,
            format!(
                "arn:aws:apigateway:{REGION}:{}:/portalproducts/{product_id}\
                 /productrestendpointpages/{page_id}",
                ::engine::emulator_account_id()
            ),
            "case {:?}: expected an ARN naming the page under its product, got {created}",
            row.case
        );
        assert!(
            arn.len() >= 20,
            "case {:?}: expected an ARN of at least the modelled 20 characters, got {arn:?}",
            row.case
        );

        // `LastModified` reads the write instant.
        let now = ::std::time::SystemTime::now()
            .duration_since(::std::time::UNIX_EPOCH)
            .map(|since| since.as_secs_f64())
            .unwrap_or_default();
        let reported = instant(row.case, &created, "lastModified");
        assert!(
            (now - reported).abs() <= LAST_MODIFIED_SLACK_SECONDS,
            "case {:?}: expected LastModified to read the write instant; got {reported} \
             against a clock reading {now}",
            row.case
        );

        assert_reports_row(
            &row,
            "CreateProductRestEndpointPage",
            &created,
            &expected_endpoint,
        );

        // The same record, read back.
        let (status, fetched) =
            call(&emulator, "GET", &format!("{collection}/{page_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetProductRestEndpointPage, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            as_str(&fetched, "productRestEndpointPageId"),
            page_id,
            "case {:?}: expected the stored page's own id, got {fetched}",
            row.case
        );
        assert_eq!(
            as_str(&fetched, "productRestEndpointPageArn"),
            arn,
            "case {:?}: expected the stored ARN, got {fetched}",
            row.case
        );
        assert_eq!(
            instant(row.case, &fetched, "lastModified"),
            reported,
            "case {:?}: expected the stored LastModified, got {fetched}",
            row.case
        );
        assert_reports_row(
            &row,
            "GetProductRestEndpointPage",
            &fetched,
            &expected_endpoint,
        );
    }

    // Every create minted a distinct id.
    for (index, id) in minted.iter().enumerate() {
        assert_eq!(
            minted.iter().filter(|other| *other == id).count(),
            1,
            "page {index}: expected distinct ids, got {minted:?}"
        );
    }
}

#[tokio::test]
async fn the_derived_endpoint_and_arn_name_the_region_the_request_addressed() {
    let emulator = emulator_with_rest_apis();
    // ONE endpoint the account holds, documented from three Regions: the REST API is the
    // same resource whichever Region a page documenting it was created through.
    let held = held_endpoint(
        &emulator,
        &json!({
            "restApiId": "regional01",
            "stage": "prod",
            "method": "GET",
            "path": "/orders"
        }),
    )
    .await;
    // The Region comes from the request's SigV4 credential scope, so a page created
    // through a request signed for another Region carries that Region in its ARN and in
    // the invoke URL it derives — and neither follows the Region a LATER reader signs for.
    for region in ["eu-west-1", "us-east-1", "ap-southeast-2"] {
        let (status, created_product) = call_in_region(
            &emulator,
            region,
            "POST",
            PRODUCTS,
            Some(json!({ "displayName": format!("product in {region}") })),
        )
        .await;
        assert_eq!(
            status, 201,
            "region {region:?}: expected 201 from CreatePortalProduct, got {status} \
             {created_product}"
        );
        let product_id = as_str(&created_product, "portalProductId");
        let collection = pages_of(&product_id);

        let (status, created) = call_in_region(
            &emulator,
            region,
            "POST",
            &collection,
            Some(json!({
                "restEndpointIdentifier": { "identifierParts": held.parts.clone() }
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "region {region:?}: expected 201 from CreateProductRestEndpointPage, got \
             {status} {created}"
        );
        let page_id = as_str(&created, "productRestEndpointPageId");
        assert_eq!(
            content_member(&created, "endpoint"),
            Some(held.invoke_url(region)),
            "region {region:?}: expected the invoke URL to name that Region, got {created}"
        );
        assert_eq!(
            as_str(&created, "productRestEndpointPageArn"),
            format!(
                "arn:aws:apigateway:{region}:{}:/portalproducts/{product_id}\
                 /productrestendpointpages/{page_id}",
                ::engine::emulator_account_id()
            ),
            "region {region:?}: expected the ARN to name that Region and the emulator \
             account, got {created}"
        );

        let (status, fetched) = call_in_region(
            &emulator,
            "us-west-2",
            "GET",
            &format!("{collection}/{page_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region:?}: expected 200 from GetProductRestEndpointPage, got {status} \
             {fetched}"
        );
        assert_eq!(
            as_str(&fetched, "productRestEndpointPageArn"),
            as_str(&created, "productRestEndpointPageArn"),
            "region {region:?}: expected the stored ARN to keep its creation Region, got \
             {fetched}"
        );
        assert_eq!(
            content_member(&fetched, "endpoint"),
            content_member(&created, "endpoint"),
            "region {region:?}: expected the stored endpoint to keep its creation Region, \
             got {fetched}"
        );
    }
}

#[tokio::test]
async fn a_request_that_identifies_no_endpoint_is_refused_and_persists_nothing() {
    let emulator = emulator_with_rest_apis();
    let product_id = product(&emulator, "Refusals").await;
    let collection = pages_of(&product_id);
    // One endpoint the account DOES hold, so a row that is refused for something else is
    // not also refused for naming an endpoint that does not exist.
    let held = held_endpoint(
        &emulator,
        &json!({
            "restApiId": "refusals01",
            "stage": "prod",
            "method": "GET",
            "path": "/orders"
        }),
    )
    .await;

    let rows: Vec<(&str, Value)> = vec![
        (
            "an identifier naming no parts",
            json!({ "restEndpointIdentifier": {} }),
        ),
        (
            // The rule cause 8 of the issue states: a page documents an endpoint the
            // ACCOUNT has, so one naming a REST API nothing created identifies nothing.
            "an identifier naming a REST API the account does not have",
            json!({
                "restEndpointIdentifier": {
                    "identifierParts": {
                        "restApiId": "nosuchrest",
                        "stage": "prod",
                        "method": "GET",
                        "path": "/orders"
                    }
                }
            }),
        ),
        (
            "an identifier naming a stage the REST API does not have",
            json!({
                "restEndpointIdentifier": {
                    "identifierParts": {
                        "restApiId": held.rest_api_id.clone(),
                        "stage": "nosuchstage",
                        "method": "GET",
                        "path": "/orders"
                    }
                }
            }),
        ),
        (
            "display content naming neither alternative",
            json!({
                "restEndpointIdentifier": { "identifierParts": held.parts.clone() },
                "displayContent": {}
            }),
        ),
        (
            "display content naming both alternatives",
            json!({
                "restEndpointIdentifier": { "identifierParts": held.parts.clone() },
                "displayContent": { "none": {}, "overrides": { "body": "both" } }
            }),
        ),
        (
            "an identifier whose parts are incomplete",
            json!({
                "restEndpointIdentifier": {
                    "identifierParts": { "restApiId": "incomplete", "stage": "prod" }
                }
            }),
        ),
        (
            "a try-it state outside the modelled catalog",
            json!({
                "restEndpointIdentifier": { "identifierParts": held.parts.clone() },
                "tryItState": "MAYBE"
            }),
        ),
    ];

    for (case, request) in rows {
        let (status, body) = call(&emulator, "POST", &collection, Some(request)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateProductRestEndpointPage, got {status} \
             {body}"
        );
        let code = error_code(&body);
        assert!(
            code == "BadRequestException" || code == "ValidationException",
            "case {case:?}: expected the request to be refused as invalid, got {code:?} {body}"
        );
    }

    // A derived endpoint is this service's own value rather than one the model bounds at
    // the boundary, and the Region it names comes from the request's credential scope, so a
    // request whose invoke URL could not be reported at all is refused rather than answered
    // with a response the model does not allow (`EndpointDisplayContentResponse$Endpoint`,
    // len 1..1024).
    let case = "a Region whose invoke URL would exceed the 1024 an endpoint may carry";
    let (status, body) = call_in_region(
        &emulator,
        &"r".repeat(1100),
        "POST",
        &collection,
        Some(json!({
            "restEndpointIdentifier": { "identifierParts": held.parts.clone() }
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 from CreateProductRestEndpointPage, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "BadRequestException",
        "case {case:?}: expected BadRequestException, got {body}"
    );

    // Every create above was refused, so the only page this product holds is the one it
    // then accepts: it reads back under the id that create answered, carrying the
    // endpoint the ACCEPTED request named and none of the refused ones'.
    let accepted = json!({
        "restEndpointIdentifier": { "identifierParts": held.parts.clone() }
    });
    let (status, created) = call(&emulator, "POST", &collection, Some(accepted)).await;
    assert_eq!(
        status, 201,
        "expected 201 for a valid page after the refusals, got {status} {created}"
    );
    let page_id = as_str(&created, "productRestEndpointPageId");
    assert_minted_id("after the refusals", "productRestEndpointPageId", &page_id);

    let (status, fetched) = call(&emulator, "GET", &format!("{collection}/{page_id}"), None).await;
    assert_eq!(
        status, 200,
        "expected 200 from GetProductRestEndpointPage for the accepted page, got {status} \
         {fetched}"
    );
    assert_eq!(
        fetched.get("restEndpointIdentifier"),
        created.get("restEndpointIdentifier"),
        "expected the accepted page to read back as it was created, got {fetched}"
    );
    assert_eq!(
        content_member(&fetched, "endpoint"),
        Some(held.invoke_url(REGION)),
        "expected the page to carry the accepted request's endpoint, got {fetched}"
    );
}

#[tokio::test]
async fn a_page_is_addressed_under_its_own_product() {
    let emulator = emulator_with_rest_apis();
    let product_id = product(&emulator, "Owner").await;
    let other_product_id = product(&emulator, "Other owner").await;
    let collection = pages_of(&product_id);

    let held = held_endpoint(
        &emulator,
        &json!({
            "restApiId": "addressed1",
            "stage": "prod",
            "method": "GET",
            "path": "/orders"
        }),
    )
    .await;
    let request = json!({
        "restEndpointIdentifier": { "identifierParts": held.parts.clone() }
    });

    // A product nothing minted has nothing to create a page under.
    for (case, id) in [
        ("a product id nothing minted", "nosuchprod0".to_string()),
        ("the product id's uppercase form", product_id.to_uppercase()),
        ("a long product id nothing minted", "z".repeat(30)),
    ] {
        let (status, body) = call(&emulator, "POST", &pages_of(&id), Some(request.clone())).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from CreateProductRestEndpointPage, got {status} \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    let (status, created) = call(&emulator, "POST", &collection, Some(request)).await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateProductRestEndpointPage, got {status} {created}"
    );
    let page_id = as_str(&created, "productRestEndpointPageId");

    // A page id nothing minted names no page, and neither does this page read under
    // ANOTHER product: a page belongs to the product it was created under.
    for (case, uri) in [
        (
            "a page id nothing minted",
            format!("{collection}/nosuchpage0"),
        ),
        (
            "the page id's uppercase form",
            format!("{collection}/{}", page_id.to_uppercase()),
        ),
        (
            "the page read under another product",
            format!("{}/{page_id}", pages_of(&other_product_id)),
        ),
        (
            "the page read under a product nothing minted",
            format!("{}/{page_id}", pages_of("nosuchprod0")),
        ),
    ] {
        let (status, body) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetProductRestEndpointPage, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // `ResourceOwnerAccountId` selects WHOSE product to read: this emulator holds its own
    // account's products, so that account finds the page and any other does not.
    let (status, mine) = call(
        &emulator,
        "GET",
        &format!(
            "{collection}/{page_id}?resourceOwnerAccountId={}",
            ::engine::emulator_account_id()
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 when the resource owner is this account, got {status} {mine}"
    );
    assert_eq!(
        as_str(&mine, "productRestEndpointPageId"),
        page_id,
        "expected the stored page, got {mine}"
    );

    for other_owner in ["111122223333", "000000000001"] {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("{collection}/{page_id}?resourceOwnerAccountId={other_owner}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "owner {other_owner:?}: expected 404 for a page owned elsewhere, got {status} \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "owner {other_owner:?}: expected NotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn the_page_status_is_owed_by_the_clock_and_reading_it_changes_nothing() {
    let emulator = emulator_with_rest_apis();
    let product_id = product(&emulator, "Clock").await;
    let collection = pages_of(&product_id);
    let held = held_endpoint(
        &emulator,
        &json!({
            "restApiId": "clocked001",
            "stage": "prod",
            "method": "GET",
            "path": "/orders"
        }),
    )
    .await;

    let (status, created) = call(
        &emulator,
        "POST",
        &collection,
        Some(json!({
            "restEndpointIdentifier": { "identifierParts": held.parts.clone() }
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateProductRestEndpointPage, got {status} {created}"
    );
    let page_id = as_str(&created, "productRestEndpointPageId");
    let uri = format!("{collection}/{page_id}");

    // The content is still being generated when the create answers, so a caller acting
    // straight after it sees the transient status.
    assert_eq!(
        as_str(&created, "status"),
        "IN_PROGRESS",
        "expected a freshly created page to report IN_PROGRESS, got {created}"
    );
    let (status, polled) = call(&emulator, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "expected 200 from GetProductRestEndpointPage, got {status} {polled}"
    );
    assert_eq!(
        as_str(&polled, "status"),
        "IN_PROGRESS",
        "expected the page to still report IN_PROGRESS within its span, got {polled}"
    );

    // Once the span the clock owes has come due, the page reports that its content was
    // generated — and stays there. Every page this service holds documents an endpoint that
    // RESOLVED, since a write naming one the account does not have is refused, so
    // `AVAILABLE` is the status one settles in. The wait is for that READING, not for a
    // fixed time.
    let settled = read_until(&emulator, "the settled page", &uri, |page| {
        page.get("status").and_then(Value::as_str) != Some("IN_PROGRESS")
    })
    .await;
    assert_eq!(
        as_str(&settled, "status"),
        "AVAILABLE",
        "expected the page to settle once its span has come due, got {settled}"
    );

    // A read is a report: a refused read of another page, and a refused create, between
    // two reads may not move what this page reports.
    let (_, _) = call(&emulator, "GET", &format!("{collection}/nosuchpage0"), None).await;
    let (_, _) = call(
        &emulator,
        "POST",
        &collection,
        Some(json!({ "restEndpointIdentifier": {} })),
    )
    .await;
    let (status, again) = call(&emulator, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "expected 200 from the second GetProductRestEndpointPage, got {status} {again}"
    );
    assert_eq!(
        settled, again,
        "expected two reads of one settled page to report the same record, got {settled} \
         then {again}"
    );
}

#[tokio::test]
async fn raw_display_content_is_reported_only_for_a_request_that_asked_for_it() {
    let emulator = emulator_with_rest_apis();
    let product_id = product(&emulator, "Raw content").await;
    let collection = pages_of(&product_id);
    let held = held_endpoint(
        &emulator,
        &json!({
            "restApiId": "rawbody001",
            "stage": "prod",
            "method": "GET",
            "path": "/orders"
        }),
    )
    .await;

    let (status, created) = call(
        &emulator,
        "POST",
        &collection,
        Some(json!({
            "restEndpointIdentifier": { "identifierParts": held.parts.clone() },
            "displayContent": { "overrides": { "body": "# Orders\n\nthe source" } }
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateProductRestEndpointPage, got {status} {created}"
    );
    let page_id = as_str(&created, "productRestEndpointPageId");

    // A flag nobody set, and one set to false, report nothing; a flag set to true reports
    // the page's content as it was supplied, in any case the caller spells it.
    for (case, query, expected) in [
        ("the flag absent", String::new(), None),
        (
            "the flag set to false",
            "?includeRawDisplayContent=false".to_string(),
            None,
        ),
        (
            "the flag set to true",
            "?includeRawDisplayContent=true".to_string(),
            Some("# Orders\n\nthe source"),
        ),
        (
            "the flag set to TRUE",
            "?includeRawDisplayContent=TRUE".to_string(),
            Some("# Orders\n\nthe source"),
        ),
    ] {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("{collection}/{page_id}{query}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetProductRestEndpointPage, got {status} {body}"
        );
        assert_eq!(
            body.get("rawDisplayContent").and_then(Value::as_str),
            expected,
            "case {case:?}: expected rawDisplayContent {expected:?}, got {body}"
        );
    }

    // A value that spells no boolean is refused rather than read as "no".
    for value in ["yes", "1", ""] {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("{collection}/{page_id}?includeRawDisplayContent={value}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "value {value:?}: expected 400 from GetProductRestEndpointPage, got {status} \
             {body}"
        );
        let code = error_code(&body);
        assert!(
            code == "BadRequestException" || code == "ValidationException",
            "value {value:?}: expected the request to be refused as invalid, got {code:?} \
             {body}"
        );
    }

    // A page whose body is API Gateway's own documentation of the REST API carries no raw
    // content, since this emulator holds no REST API documentation to read it from.
    let plain = held_endpoint(
        &emulator,
        &json!({
            "restApiId": "nobody0001",
            "stage": "prod",
            "method": "GET",
            "path": "/orders"
        }),
    )
    .await;
    let (status, no_body) = call(
        &emulator,
        "POST",
        &collection,
        Some(json!({
            "restEndpointIdentifier": { "identifierParts": plain.parts.clone() }
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateProductRestEndpointPage, got {status} {no_body}"
    );
    let no_body_id = as_str(&no_body, "productRestEndpointPageId");
    let (status, fetched) = call(
        &emulator,
        "GET",
        &format!("{collection}/{no_body_id}?includeRawDisplayContent=true"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from GetProductRestEndpointPage, got {status} {fetched}"
    );
    assert!(
        carries_nothing(&fetched, "rawDisplayContent"),
        "expected a page with no supplied body to report no rawDisplayContent, got {fetched}"
    );
}

#[tokio::test]
async fn one_product_holds_at_most_the_pages_its_bound_allows() {
    // The bound the service enforces, read from the service itself rather than copied:
    // the create past it is refused with the modelled `TooManyRequestsException` ("a
    // limit has been exceeded").
    const LIMIT: usize = service_com_amazonaws_apigatewayv2_apigatewayv2::state::
        MAX_PRODUCT_REST_ENDPOINT_PAGES_PER_PRODUCT;

    let emulator = emulator_with_rest_apis();
    let product_id = product(&emulator, "Bounded").await;
    let collection = pages_of(&product_id);
    // ONE endpoint the account holds, documented by every page: what the bound counts is
    // pages, not endpoints.
    let held = held_endpoint(
        &emulator,
        &json!({
            "restApiId": "bounded001",
            "stage": "prod",
            "method": "GET",
            "path": "/orders"
        }),
    )
    .await;

    let page_request = |index: usize| {
        let mut parts = held.parts.clone();
        parts["path"] = json!(format!("/orders/{index}"));
        json!({ "restEndpointIdentifier": { "identifierParts": parts } })
    };

    for index in 0..LIMIT {
        let (status, created) =
            call(&emulator, "POST", &collection, Some(page_request(index))).await;
        assert_eq!(
            status, 201,
            "page {index} of {LIMIT}: expected 201 from CreateProductRestEndpointPage, got \
             {status} {created}"
        );
    }

    let (status, body) = call(&emulator, "POST", &collection, Some(page_request(LIMIT))).await;
    assert_eq!(
        status, 429,
        "expected 429 from the page past the bound of {LIMIT}, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyRequestsException",
        "expected TooManyRequestsException, got {body}"
    );

    // The bound is PER PRODUCT: another product of the same account has room.
    let other_product_id = product(&emulator, "Room").await;
    let (status, created) = call(
        &emulator,
        "POST",
        &pages_of(&other_product_id),
        Some(page_request(0)),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 under another product, got {status} {created}"
    );
}
