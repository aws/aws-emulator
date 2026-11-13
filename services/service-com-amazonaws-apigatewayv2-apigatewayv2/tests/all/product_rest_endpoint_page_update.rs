// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `UpdateProductRestEndpointPage`.
//!
//! The rules these cases assert:
//!
//! * **The update is a PATCH**
//!   (`PATCH /v2/portalproducts/{PortalProductId}/productrestendpointpages/{Id}`): a
//!   member the request carries replaces the stored one, a member it omits leaves the
//!   stored value exactly as it was, and the members this operation does not take (the
//!   page's id, its ARN and the REST endpoint it documents) are carried over untouched.
//!   What the response reports is what a later `GetProductRestEndpointPage` reports.
//! * **A `DisplayContent` the patch carries replaces the page's content wholesale**: each
//!   member it overrides takes the overriding value, and each member it does not override
//!   returns to the default API Gateway derives from the REST endpoint the page documents
//!   — the `Endpoint` to that endpoint's own invoke URL
//!   (`https://{restApiId}.execute-api.{region}.amazonaws.com/{stage}/{path}`), and
//!   `Body` and `OperationName` to nothing, since their defaults live in the REST API this
//!   emulator does not hold.
//! * **That derived default names the page's OWN Region**, the one its ARN names, not the
//!   Region the patching caller signed for.
//! * **`LastModified` advances when the patch carries a member to apply**, and a patch
//!   that carries none leaves the record — `LastModified` included — exactly as it was.
//! * **A page is patched only through its own product and its own minted id**: an unknown
//!   product, an unknown page id and a page addressed under another product are each 404
//!   `NotFoundException` and write nothing.
//! * **A patch whose values are invalid is refused 400 and persists nothing**: a
//!   `DisplayContent` naming neither or both of its alternatives (`None` / `Overrides`),
//!   a `TryItState` outside its catalog (`ENABLED | DISABLED`), and a value past a
//!   modelled bound (`DisplayContentOverrides$Body` len 1..32768, `$Endpoint` len 1..1024,
//!   `$OperationName` len 1..255).
//!
//! The rows vary what the rules generalise over: WHICH members the patch carries (each
//! one alone, both, none), which alternative its display content names, how many members
//! the overrides carry, the value's length at and past each modelled bound, its script,
//! whether the member was set before the patch, whether the patch repeats the stored
//! value, and the shape of the endpoint the page documents. The recorded lifecycle that
//! filed the issue — create, patch, read back — is ONE row of the first table.

use serde_json::{json, Value};

use crate::api_lifecycle::{
    as_str, call, call_in_region, carries_nothing, emulator_with_rest_apis, error_code, instant,
    past_the_whole_second_of, read_until, rest_endpoint,
};
use crate::product_rest_endpoint_page_lifecycle::{
    content_member, held_endpoint, invoke_url, max_body, max_stage, pages_of, product, PRODUCTS,
    REGION,
};

/// One patch row: the page to create, the patch under test, and what the patched page
/// must report.
struct Row {
    case: &'static str,
    /// `RestEndpointIdentifier$IdentifierParts$RestApiId` of the endpoint the page
    /// documents (len 1..50) — written as a PLACEHOLDER naming the REST API each case
    /// creates through the service that owns REST APIs, and replaced by the id that service
    /// minted before the page is created.
    rest_api_id: String,
    /// `IdentifierParts$Stage` (len 1..128).
    stage: String,
    /// `IdentifierParts$Method` (len 1..20).
    method: &'static str,
    /// `IdentifierParts$Path` (len 1..4096).
    path: &'static str,
    /// The create's `DisplayContent`, absent for a create that named none.
    created_content: Option<Value>,
    /// The create's `TryItState`, absent for a create that named none.
    created_try_it_state: Option<&'static str>,
    /// The patch under test.
    patch: Value,
    /// The `displayContent.endpoint` the patched page must report, or `None` for the
    /// default derived from the documented REST endpoint.
    expected_endpoint: Option<String>,
    /// The `displayContent.body` the patched page must report, if any.
    expected_body: Option<String>,
    /// The `displayContent.operationName` the patched page must report, if any.
    expected_operation_name: Option<String>,
    /// The `tryItState` the patched page must report.
    expected_try_it_state: &'static str,
}

impl Row {
    /// The `RestEndpointIdentifier$IdentifierParts` of the endpoint this row's page
    /// documents.
    fn parts(&self) -> Value {
        json!({
            "restApiId": self.rest_api_id,
            "stage": self.stage,
            "method": self.method,
            "path": self.path
        })
    }

    /// The request that creates this row's page.
    fn create(&self) -> Value {
        let mut request = json!({ "restEndpointIdentifier": { "identifierParts": self.parts() } });
        if let Some(content) = &self.created_content {
            request["displayContent"] = content.clone();
        }
        if let Some(state) = self.created_try_it_state {
            request["tryItState"] = json!(state);
        }
        request
    }

    /// The `displayContent.endpoint` the patched page must report: the row's expectation,
    /// else the invoke URL of the endpoint it documents, in the Region the page was
    /// created in.
    fn endpoint(&self) -> String {
        self.expected_endpoint
            .clone()
            .unwrap_or_else(|| invoke_url(&self.rest_api_id, &self.stage, REGION))
    }
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the try-it state alone, leaving the content the create resolved",
            rest_api_id: "a1b2c3d4e5".to_string(),
            stage: "prod".to_string(),
            method: "GET",
            path: "/orders",
            created_content: Some(json!({ "overrides": { "body": "the created body" } })),
            created_try_it_state: Some("DISABLED"),
            patch: json!({ "tryItState": "ENABLED" }),
            expected_endpoint: None,
            expected_body: Some("the created body".to_string()),
            expected_operation_name: None,
            expected_try_it_state: "ENABLED",
        },
        Row {
            case: "the try-it state moved back to DISABLED",
            rest_api_id: "backagain1".to_string(),
            stage: "prod".to_string(),
            method: "GET",
            path: "/orders",
            created_content: None,
            created_try_it_state: Some("ENABLED"),
            patch: json!({ "tryItState": "DISABLED" }),
            expected_endpoint: None,
            expected_body: None,
            expected_operation_name: None,
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "display content alone: one override replaces, the rest return to their \
                   defaults",
            rest_api_id: "zzz999".to_string(),
            stage: "test".to_string(),
            method: "POST",
            path: "/orders",
            created_content: Some(json!({
                "overrides": {
                    "body": "the created body",
                    "endpoint": "https://created.example.com/orders",
                    "operationName": "CreatedName"
                }
            })),
            created_try_it_state: Some("ENABLED"),
            patch: json!({ "displayContent": { "overrides": { "operationName": "PatchedName" } } }),
            expected_endpoint: None,
            expected_body: None,
            expected_operation_name: Some("PatchedName".to_string()),
            // Not an input of this patch, so the stored value stands.
            expected_try_it_state: "ENABLED",
        },
        Row {
            case: "the `none` alternative, which returns every member to its default",
            rest_api_id: "noneagain1".to_string(),
            stage: "v1".to_string(),
            method: "PUT",
            path: "/orders/{orderId}",
            created_content: Some(json!({
                "overrides": {
                    "body": "documentation",
                    "endpoint": "https://created.example.com/v1/orders",
                    "operationName": "PutOrder"
                }
            })),
            created_try_it_state: Some("ENABLED"),
            patch: json!({ "displayContent": { "none": {} } }),
            expected_endpoint: None,
            expected_body: None,
            expected_operation_name: None,
            expected_try_it_state: "ENABLED",
        },
        Row {
            case: "both members at once, onto a page created with neither",
            rest_api_id: "bothmembr1".to_string(),
            stage: "beta".to_string(),
            method: "DELETE",
            path: "/orders/{orderId}",
            created_content: None,
            created_try_it_state: None,
            patch: json!({
                "displayContent": {
                    "overrides": {
                        "body": "# Orders\n\nRemove one order.",
                        "endpoint": "https://api.example.com/v1/orders",
                        "operationName": "DeleteOrder"
                    }
                },
                "tryItState": "ENABLED"
            }),
            expected_endpoint: Some("https://api.example.com/v1/orders".to_string()),
            expected_body: Some("# Orders\n\nRemove one order.".to_string()),
            expected_operation_name: Some("DeleteOrder".to_string()),
            expected_try_it_state: "ENABLED",
        },
        Row {
            case: "an empty overrides structure, which overrides nothing",
            rest_api_id: "emptyover2".to_string(),
            stage: "prod".to_string(),
            method: "HEAD",
            path: "/health",
            created_content: Some(json!({ "overrides": { "body": "the created body" } })),
            created_try_it_state: None,
            patch: json!({ "displayContent": { "overrides": {} } }),
            expected_endpoint: None,
            expected_body: None,
            expected_operation_name: None,
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "content outside ASCII, which round-trips unchanged",
            rest_api_id: "unicode002".to_string(),
            stage: "prod".to_string(),
            method: "GET",
            path: "/注文",
            created_content: None,
            created_try_it_state: None,
            patch: json!({
                "displayContent": {
                    "overrides": { "body": "注文 API — ünïcödé 🎉", "operationName": "注文を取得" }
                }
            }),
            expected_endpoint: None,
            expected_body: Some("注文 API — ünïcödé 🎉".to_string()),
            expected_operation_name: Some("注文を取得".to_string()),
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "an overriding body at the modelled maximum length",
            rest_api_id: "maxbody002".to_string(),
            stage: "prod".to_string(),
            method: "GET",
            path: "/orders",
            created_content: None,
            created_try_it_state: None,
            patch: json!({ "displayContent": { "overrides": { "body": max_body() } } }),
            expected_endpoint: None,
            expected_body: Some(max_body()),
            expected_operation_name: None,
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "an overriding operation name of one character, the modelled minimum",
            rest_api_id: "minopname1".to_string(),
            stage: "prod".to_string(),
            method: "GET",
            path: "/orders",
            created_content: Some(json!({ "overrides": { "operationName": "GetOrders" } })),
            created_try_it_state: None,
            patch: json!({ "displayContent": { "overrides": { "operationName": "x" } } }),
            expected_endpoint: None,
            expected_body: None,
            expected_operation_name: Some("x".to_string()),
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "a stage at its modelled maximum, so the default is re-derived from the \
                   page's own endpoint",
            rest_api_id: "maxstage02".to_string(),
            stage: max_stage(),
            method: "GET",
            path: "/orders",
            created_content: Some(json!({
                "overrides": { "endpoint": "https://created.example.com/orders" }
            })),
            created_try_it_state: None,
            patch: json!({ "displayContent": { "none": {} } }),
            expected_endpoint: None,
            expected_body: None,
            expected_operation_name: None,
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "a path written without its leading separator, re-derived with one \
                   separator",
            rest_api_id: "noslash002".to_string(),
            stage: "prod".to_string(),
            method: "GET",
            path: "orders",
            created_content: Some(json!({
                "overrides": { "endpoint": "https://created.example.com/orders" }
            })),
            created_try_it_state: None,
            patch: json!({ "displayContent": { "none": {} } }),
            expected_endpoint: None,
            expected_body: None,
            expected_operation_name: None,
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "the root path",
            rest_api_id: "rootpath02".to_string(),
            stage: "prod".to_string(),
            method: "GET",
            path: "/",
            created_content: None,
            created_try_it_state: None,
            patch: json!({ "displayContent": { "overrides": { "body": "the root" } } }),
            expected_endpoint: None,
            expected_body: Some("the root".to_string()),
            expected_operation_name: None,
            expected_try_it_state: "DISABLED",
        },
        Row {
            case: "the values the page already carries, repeated",
            rest_api_id: "idempot001".to_string(),
            stage: "prod".to_string(),
            method: "GET",
            path: "/orders",
            created_content: Some(json!({ "overrides": { "body": "unchanged" } })),
            created_try_it_state: Some("ENABLED"),
            patch: json!({
                "displayContent": { "overrides": { "body": "unchanged" } },
                "tryItState": "ENABLED"
            }),
            expected_endpoint: None,
            expected_body: Some("unchanged".to_string()),
            expected_operation_name: None,
            expected_try_it_state: "ENABLED",
        },
    ]
}

/// Read one page back through `GetProductRestEndpointPage`, asserting the read succeeded.
async fn get_page(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    uri: &str,
) -> Value {
    let (status, fetched) = call(emulator, "GET", uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetProductRestEndpointPage, got {status} {fetched}"
    );
    fetched
}

/// Create one product and one page under it, and answer `(page uri, the created page)`.
async fn page_under_a_fresh_product(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    create: Value,
) -> (String, Value) {
    let product_id = product(emulator, case).await;
    let collection = pages_of(&product_id);
    let (status, created) = call(emulator, "POST", &collection, Some(create)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateProductRestEndpointPage, got {status} {created}"
    );
    let page_id = as_str(&created, "productRestEndpointPageId");
    (format!("{collection}/{page_id}"), created)
}

/// The members of a page that no patch of this operation can move, so two readings of one
/// page are compared through ONE projection.
fn identity(document: &Value) -> Value {
    json!({
        "productRestEndpointPageId": document.get("productRestEndpointPageId"),
        "productRestEndpointPageArn": document.get("productRestEndpointPageArn"),
        "restEndpointIdentifier": document.get("restEndpointIdentifier")
    })
}

/// The members of a page a patch CAN move, beside its modification instant.
fn patchable(document: &Value) -> Value {
    json!({
        "displayContent": document.get("displayContent"),
        "tryItState": document.get("tryItState")
    })
}

#[tokio::test]
async fn a_patch_replaces_the_members_it_carries_and_leaves_the_others_as_they_were() {
    // A page documents an endpoint the account HOLDS, so the service that owns REST APIs is
    // mounted beside this one and each row's endpoint is created through it.
    let emulator = emulator_with_rest_apis();

    for mut row in rows() {
        row.rest_api_id = rest_endpoint(&emulator, &row.rest_api_id, &row.stage).await;
        let (uri, created) = page_under_a_fresh_product(&emulator, row.case, row.create()).await;
        let before = get_page(&emulator, row.case, &uri).await;

        let (status, patched) = call(&emulator, "PATCH", &uri, Some(row.patch.clone())).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from UpdateProductRestEndpointPage, got {status} {patched}",
            row.case
        );

        // The content the patch resolved: each overridden member takes its value, each
        // member the patch left out takes the default derived from the documented REST
        // endpoint.
        let expected_endpoint = row.endpoint();
        assert_eq!(
            content_member(&patched, "endpoint"),
            Some(expected_endpoint.clone()),
            "case {:?}: expected the patched page to report the endpoint \
             {expected_endpoint:?}, got {patched}",
            row.case
        );
        assert_eq!(
            content_member(&patched, "body"),
            row.expected_body.clone(),
            "case {:?}: expected the patched page to report body {:?}, got {patched}",
            row.case,
            row.expected_body
        );
        assert_eq!(
            content_member(&patched, "operationName"),
            row.expected_operation_name.clone(),
            "case {:?}: expected the patched page to report operationName {:?}, got {patched}",
            row.case,
            row.expected_operation_name
        );
        assert_eq!(
            as_str(&patched, "tryItState"),
            row.expected_try_it_state,
            "case {:?}: expected the patched page to report tryItState {:?}, got {patched}",
            row.case,
            row.expected_try_it_state
        );

        // A patch re-generates the page's content, so the page is back to IN_PROGRESS
        // until that generation comes due — and a status still in flight has nothing to
        // explain, so it carries no status exception.
        let status_value = as_str(&patched, "status");
        assert_eq!(
            status_value, "IN_PROGRESS",
            "case {:?}: expected the patched page to report IN_PROGRESS, got {patched}",
            row.case
        );
        assert!(
            carries_nothing(&patched, "statusException"),
            "case {:?}: expected a page whose generation is in flight to report no \
             statusException, got {patched}",
            row.case
        );

        // The members this operation does not take are carried over untouched.
        assert_eq!(
            identity(&patched),
            identity(&created),
            "case {:?}: expected the page's id, ARN and documented endpoint to survive the \
             patch, got {patched} against {created}",
            row.case
        );

        // What the response reports is what a later reader reports: the patch was
        // persisted, not echoed.
        let fetched = get_page(&emulator, row.case, &uri).await;
        assert_eq!(
            patchable(&fetched),
            patchable(&patched),
            "case {:?}: expected GetProductRestEndpointPage to report the patched content, \
             got {fetched} against {patched}",
            row.case
        );
        assert_eq!(
            identity(&fetched),
            identity(&patched),
            "case {:?}: expected the stored id, ARN and documented endpoint, got {fetched} \
             against {patched}",
            row.case
        );
        assert_eq!(
            instant(row.case, &fetched, "lastModified"),
            instant(row.case, &patched, "lastModified"),
            "case {:?}: expected the stored LastModified, got {fetched} against {patched}",
            row.case
        );

        // `LastModified` reads this patch's own clock rather than a fixed literal, and
        // never sits before the instant the page was created at.
        assert!(
            instant(row.case, &patched, "lastModified")
                >= instant(row.case, &before, "lastModified"),
            "case {:?}: expected LastModified not to move backwards, got {patched} against \
             {before}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_patch_that_carries_a_member_advances_last_modified() {
    let emulator = emulator_with_rest_apis();
    let held = held_endpoint(
        &emulator,
        &json!({
            "restApiId": "clockpage1",
            "stage": "prod",
            "method": "GET",
            "path": "/orders"
        }),
    )
    .await;

    // One row per patchable member, so neither of them is the only one that records a
    // modification.
    let patches = [
        (
            "the display content",
            json!({ "displayContent": { "overrides": { "body": "moved" } } }),
        ),
        ("the try-it state", json!({ "tryItState": "ENABLED" })),
    ];

    for (case, patch) in patches {
        let (uri, created) = page_under_a_fresh_product(
            &emulator,
            case,
            json!({
                "restEndpointIdentifier": { "identifierParts": held.parts.clone() }
            }),
        )
        .await;
        let created_at = instant(case, &created, "lastModified");

        // The member is reported at whole-second granularity, so the patch has to land in a
        // LATER second for a move to be visible at all: the wait is for the clock to read
        // one, never for a fixed time.
        past_the_whole_second_of(case, created_at).await;

        let (status, patched) = call(&emulator, "PATCH", &uri, Some(patch)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateProductRestEndpointPage, got {status} \
             {patched}"
        );
        let patched_at = instant(case, &patched, "lastModified");
        assert!(
            patched_at > created_at,
            "case {case:?}: expected LastModified to advance past the creation instant \
             {created_at}, got {patched_at}"
        );

        let fetched = get_page(&emulator, case, &uri).await;
        assert_eq!(
            instant(case, &fetched, "lastModified"),
            patched_at,
            "case {case:?}: expected GetProductRestEndpointPage to report the patched \
             LastModified, got {fetched}"
        );
    }
}

#[tokio::test]
async fn a_patch_that_carries_no_member_leaves_the_record_exactly_as_it_was() {
    let emulator = emulator_with_rest_apis();
    let case = "a patch carrying none of the page's members";
    let held = held_endpoint(
        &emulator,
        &json!({
            "restApiId": "untouched1",
            "stage": "prod",
            "method": "GET",
            "path": "/orders"
        }),
    )
    .await;

    let (uri, created) = page_under_a_fresh_product(
        &emulator,
        case,
        json!({
            "restEndpointIdentifier": { "identifierParts": held.parts.clone() },
            "displayContent": { "overrides": { "body": "nothing in the patch applies" } },
            "tryItState": "ENABLED"
        }),
    )
    .await;

    // Two conditions this case needs before it reads the record it will compare against,
    // each waited for rather than timed: the clock has to reach a LATER whole second than
    // the create's, since the member is reported at whole-second granularity and only a
    // later reading could show a move at all; and the page's `Status` has to be settled, so
    // both readings report the same one.
    past_the_whole_second_of(case, instant(case, &created, "lastModified")).await;
    let before = read_until(&emulator, case, &uri, |page| {
        page.get("status").and_then(Value::as_str) != Some("IN_PROGRESS")
    })
    .await;

    let (status, patched) = call(&emulator, "PATCH", &uri, Some(json!({}))).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from UpdateProductRestEndpointPage, got {status} {patched}"
    );
    assert_eq!(
        patchable(&patched),
        patchable(&before),
        "case {case:?}: expected the content to be reported unchanged, got {patched} against \
         {before}"
    );
    assert_eq!(
        instant(case, &patched, "lastModified"),
        instant(case, &before, "lastModified"),
        "case {case:?}: expected LastModified to stand still, got {patched} against {before}"
    );

    let after = get_page(&emulator, case, &uri).await;
    assert_eq!(
        patchable(&after),
        patchable(&before),
        "case {case:?}: expected the stored content to be unchanged, got {after} against {before}"
    );
    assert_eq!(
        instant(case, &after, "lastModified"),
        instant(case, &before, "lastModified"),
        "case {case:?}: expected the stored LastModified to be unchanged, got {after} against \
         {before}"
    );
}

#[tokio::test]
async fn the_re_derived_endpoint_names_the_region_the_page_was_created_in() {
    let emulator = emulator_with_rest_apis();
    let held = held_endpoint(
        &emulator,
        &json!({
            "restApiId": "regional02",
            "stage": "prod",
            "method": "GET",
            "path": "/orders"
        }),
    )
    .await;

    // The page's Region comes from the request that created it, so a patch signed for
    // ANOTHER Region re-derives the default endpoint in the page's own Region — the one
    // its ARN names.
    for (home, patched_from) in [
        ("ap-southeast-2", "us-east-1"),
        ("us-east-1", "eu-west-1"),
        ("eu-west-1", "ap-southeast-2"),
    ] {
        let case = home;
        let (status, created_product) = call_in_region(
            &emulator,
            home,
            "POST",
            PRODUCTS,
            Some(json!({ "displayName": format!("product in {home}") })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreatePortalProduct, got {status} \
             {created_product}"
        );
        let product_id = as_str(&created_product, "portalProductId");
        let collection = pages_of(&product_id);

        let (status, created) = call_in_region(
            &emulator,
            home,
            "POST",
            &collection,
            Some(json!({
                "restEndpointIdentifier": { "identifierParts": held.parts.clone() },
                "displayContent": { "overrides": { "endpoint": "https://created.example.com" } }
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateProductRestEndpointPage, got {status} \
             {created}"
        );
        let page_id = as_str(&created, "productRestEndpointPageId");
        let uri = format!("{collection}/{page_id}");

        let (status, patched) = call_in_region(
            &emulator,
            patched_from,
            "PATCH",
            &uri,
            Some(json!({ "displayContent": { "none": {} } })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateProductRestEndpointPage signed for \
             {patched_from:?}, got {status} {patched}"
        );
        assert_eq!(
            content_member(&patched, "endpoint"),
            Some(held.invoke_url(home)),
            "case {case:?}: expected the re-derived invoke URL to name the page's own \
             Region, got {patched}"
        );
        assert_eq!(
            as_str(&patched, "productRestEndpointPageArn"),
            as_str(&created, "productRestEndpointPageArn"),
            "case {case:?}: expected the stored ARN to survive a patch signed elsewhere, got \
             {patched}"
        );
    }
}

#[tokio::test]
async fn a_page_is_patched_only_through_its_own_product_and_minted_id() {
    let emulator = emulator_with_rest_apis();
    let case = "a patch addressed at a page nothing minted";
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

    let holder = product(&emulator, "Addressed").await;
    let collection = pages_of(&holder);
    let (status, created) = call(
        &emulator,
        "POST",
        &collection,
        Some(json!({
            "restEndpointIdentifier": { "identifierParts": held.parts.clone() },
            "displayContent": { "overrides": { "body": "the only page" } }
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateProductRestEndpointPage, got {status} {created}"
    );
    let page_id = as_str(&created, "productRestEndpointPageId");
    let uri = format!("{collection}/{page_id}");
    let before = get_page(&emulator, case, &uri).await;

    // A second product, which does not hold the page above.
    let other_product = product(&emulator, "Another product").await;

    let misaddressed: Vec<(&str, String)> = vec![
        (
            "a product nothing minted",
            format!("{}/{page_id}", pages_of("nosuchprod0")),
        ),
        (
            "a page id nothing minted",
            format!("{collection}/nosuchpage0"),
        ),
        (
            "the page id's uppercase form",
            format!("{collection}/{}", page_id.to_uppercase()),
        ),
        (
            "the page id with its first character dropped",
            format!("{collection}/{}", &page_id[1..]),
        ),
        (
            "a page id at the modelled maximum that nothing minted",
            format!("{collection}/{}", "z".repeat(30)),
        ),
        (
            "the page addressed under another product",
            format!("{}/{page_id}", pages_of(&other_product)),
        ),
    ];

    for (label, uri) in misaddressed {
        let (status, body) = call(
            &emulator,
            "PATCH",
            &uri,
            Some(json!({ "tryItState": "ENABLED" })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 from UpdateProductRestEndpointPage, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {label:?}: expected NotFoundException, got {body}"
        );
    }

    // None of those refusals wrote anything: the page that does exist is unchanged.
    let after = get_page(&emulator, case, &uri).await;
    assert_eq!(
        patchable(&after),
        patchable(&before),
        "case {case:?}: expected the existing page to be untouched by the refused patches, got \
         {after} against {before}"
    );
    assert_eq!(
        instant(case, &after, "lastModified"),
        instant(case, &before, "lastModified"),
        "case {case:?}: expected the existing page's LastModified to be untouched, got {after} \
         against {before}"
    );
}

#[tokio::test]
async fn a_patch_whose_values_are_invalid_is_refused_and_persists_nothing() {
    let emulator = emulator_with_rest_apis();
    let case = "an invalid patch";
    let held = held_endpoint(
        &emulator,
        &json!({
            "restApiId": "validpage1",
            "stage": "prod",
            "method": "GET",
            "path": "/orders"
        }),
    )
    .await;

    let (uri, _created) = page_under_a_fresh_product(
        &emulator,
        case,
        json!({
            "restEndpointIdentifier": { "identifierParts": held.parts.clone() },
            "displayContent": { "overrides": { "body": "a valid record" } },
            "tryItState": "ENABLED"
        }),
    )
    .await;
    let before = get_page(&emulator, case, &uri).await;

    // `DisplayContent` is a choice between `None` and `Overrides`, so one naming neither
    // or both describes no content; `TryItState` is `ENABLED | DISABLED`; and the
    // overrides carry modelled bounds (`Body` len 1..32768, `Endpoint` len 1..1024,
    // `OperationName` len 1..255). Each row is past exactly one of those, including one
    // character past each published maximum.
    let rows: Vec<(&str, Value)> = vec![
        (
            "display content naming neither alternative",
            json!({ "displayContent": {} }),
        ),
        (
            "display content naming both alternatives",
            json!({ "displayContent": { "none": {}, "overrides": { "body": "both" } } }),
        ),
        (
            "a try-it state outside the modelled catalog",
            json!({ "tryItState": "MAYBE" }),
        ),
        (
            "a try-it state in the wrong case",
            json!({ "tryItState": "enabled" }),
        ),
        (
            "an empty overriding body",
            json!({ "displayContent": { "overrides": { "body": "" } } }),
        ),
        (
            "an overriding body one character past the maximum",
            json!({ "displayContent": { "overrides": { "body": "b".repeat(32769) } } }),
        ),
        (
            "an empty overriding endpoint",
            json!({ "displayContent": { "overrides": { "endpoint": "" } } }),
        ),
        (
            "an overriding endpoint one character past the maximum",
            json!({ "displayContent": { "overrides": { "endpoint": "e".repeat(1025) } } }),
        ),
        (
            "an overriding operation name one character past the maximum",
            json!({ "displayContent": { "overrides": { "operationName": "o".repeat(256) } } }),
        ),
        (
            "a valid try-it state beside an invalid display content",
            json!({ "displayContent": {}, "tryItState": "DISABLED" }),
        ),
    ];

    for (label, patch) in rows {
        let (status, body) = call(&emulator, "PATCH", &uri, Some(patch)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 from UpdateProductRestEndpointPage, got {status} {body}"
        );
        let code = error_code(&body);
        assert!(
            code == "BadRequestException" || code == "ValidationException",
            "case {label:?}: expected the patch to be refused as invalid, got {code:?} {body}"
        );

        let after = get_page(&emulator, label, &uri).await;
        assert_eq!(
            patchable(&after),
            patchable(&before),
            "case {label:?}: expected a refused patch to persist nothing, got {after} against \
             {before}"
        );
        assert_eq!(
            instant(label, &after, "lastModified"),
            instant(label, &before, "lastModified"),
            "case {label:?}: expected a refused patch to leave LastModified alone, got {after} \
             against {before}"
        );
    }
}
