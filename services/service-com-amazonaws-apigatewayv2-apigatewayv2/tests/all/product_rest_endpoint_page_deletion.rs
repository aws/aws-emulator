// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `DeleteProductRestEndpointPage`.
//!
//! The rules these cases assert:
//!
//! * **A page its portal product holds is removed**: the delete answers 204 with an empty
//!   body, `GetProductRestEndpointPage` then reports 404 `NotFoundException`, and a second
//!   delete of the same id answers the same empty 204 — the delete is idempotent, so a page
//!   that is already gone is already in the state the call asks for. The id the page held is
//!   free again, and a page created afterwards is a NEW page with an id of its own.
//! * **The delete reaches exactly one page**: every sibling page of the same product
//!   survives and still reports its own content, and so do the product itself, the product
//!   pages it holds and its sharing policy — a page is a child of its product, not the
//!   other way round.
//! * **A page is addressed by its product and its own minted id, not by the Region the
//!   caller signs for**: a page created through a request signed for one Region is deleted
//!   through a request signed for another, the way `GetProductRestEndpointPage` reads one.
//! * **The PRODUCT is the resource that has to exist**: a `PortalProductId` nothing minted
//!   is a 404 `NotFoundException` that removes nothing, while a page id that resolves to
//!   nothing UNDER a product the account holds is a 204 that removes nothing — a page id
//!   nothing minted, the id's uppercase form, and a page of one product addressed under
//!   another.
//!
//! The rows vary what the rules generalise over: which optional members the deleted page
//! was created with, which alternative its display content named, the stage, method and
//! path of the REST endpoint it documents, how many sibling pages the product holds, and
//! which other sub-resources the product carries. The recorded lifecycle that filed the
//! issue — one page created under one product and then deleted — is ONE row of the first
//! table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{
    as_str, assert_minted_id, call, call_in_region, emulator_with_rest_apis, error_code,
};
use crate::product_rest_endpoint_page_lifecycle::{
    content_member, held_endpoint, pages_of, product, PRODUCTS, REGION,
};

/// The identifier parts of one REST endpoint
/// (`RestEndpointIdentifier$IdentifierParts`: a REST API, a stage, a method and a path).
fn parts(rest_api_id: &str, stage: &str, method: &str, path: &str) -> Value {
    json!({
        "restApiId": rest_api_id,
        "stage": stage,
        "method": method,
        "path": path
    })
}

/// Create one page under `portal_product_id` and answer its `ProductRestEndpointPageId`.
async fn create_page(
    emulator: &MultiServiceRouter,
    case: &str,
    portal_product_id: &str,
    request: Value,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &pages_of(portal_product_id),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateProductRestEndpointPage, got {status} {created}"
    );
    as_str(&created, "productRestEndpointPageId")
}

/// `(status, body)` of `GetProductRestEndpointPage`.
async fn get_page(
    emulator: &MultiServiceRouter,
    portal_product_id: &str,
    page_id: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("{}/{page_id}", pages_of(portal_product_id)),
        None,
    )
    .await
}

/// `(status, body)` of `DeleteProductRestEndpointPage`.
async fn delete_page(
    emulator: &MultiServiceRouter,
    portal_product_id: &str,
    page_id: &str,
) -> (u16, Value) {
    call(
        emulator,
        "DELETE",
        &format!("{}/{page_id}", pages_of(portal_product_id)),
        None,
    )
    .await
}

/// One row of the "the delete goes through" table: the page being deleted, the sibling
/// pages created beside it, and the other sub-resources its product carries.
struct Row {
    case: &'static str,
    /// The REST endpoint the deleted page documents.
    parts: Value,
    /// The deleted page's `DisplayContent`, absent for a create that named none.
    display_content: Option<Value>,
    /// The deleted page's `TryItState`, absent for a create that named none.
    try_it_state: Option<&'static str>,
    /// The REST endpoints the sibling pages document, none of which this row deletes.
    siblings: Vec<Value>,
    /// `DisplayContent$Title`s of the product pages the product also holds, which a page
    /// delete must not touch.
    product_page_titles: &'static [&'static str],
    /// The sharing policy the product carries, if any.
    sharing_policy: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the only page of its product, created with every default",
            parts: parts("a1b2c3d4e5", "prod", "GET", "/orders"),
            display_content: None,
            try_it_state: None,
            siblings: Vec::new(),
            product_page_titles: &[],
            sharing_policy: None,
        },
        Row {
            case: "a page carrying every display-content override and an enabled try-it \
                   state, beside one sibling",
            parts: parts("overrides1", "v1", "PUT", "/orders/{orderId}"),
            display_content: Some(json!({
                "overrides": {
                    "body": "# Orders\n\nReplace one order.",
                    "endpoint": "https://api.example.com/v1/orders",
                    "operationName": "PutOrder"
                }
            })),
            try_it_state: Some("ENABLED"),
            siblings: vec![parts("sibling001", "prod", "GET", "/orders")],
            product_page_titles: &[],
            sharing_policy: None,
        },
        Row {
            case: "a page whose display content named the `none` alternative, beside \
                   several siblings",
            parts: parts("nonealt001", "test", "POST", "/orders"),
            display_content: Some(json!({ "none": {} })),
            try_it_state: Some("DISABLED"),
            siblings: vec![
                parts("sibling002", "prod", "GET", "/orders"),
                parts("sibling003", "beta", "DELETE", "/orders/{orderId}"),
                parts("sibling004", "prod", "HEAD", "/health"),
            ],
            product_page_titles: &[],
            sharing_policy: None,
        },
        Row {
            case: "a page of a product that also holds product pages and a sharing policy",
            parts: parts("withothers", "prod", "PATCH", "/orders"),
            display_content: Some(json!({ "overrides": { "body": "documentation" } })),
            try_it_state: None,
            siblings: vec![parts("sibling005", "prod", "GET", "/orders")],
            product_page_titles: &["Getting started", "Reference"],
            sharing_policy: Some(
                "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                 \"Principal\":{\"AWS\":\"*\"},\"Action\":\"apigateway:GET\"}]}",
            ),
        },
        Row {
            case: "a page documenting the root path, with a body override outside ASCII",
            parts: parts("rootpath02", "prod", "GET", "/"),
            display_content: Some(json!({
                "overrides": { "body": "注文 API — ünïcödé 🎉", "operationName": "注文を取得" }
            })),
            try_it_state: Some("ENABLED"),
            siblings: vec![parts("sibling006", "prod", "GET", "orders")],
            product_page_titles: &["Overview"],
            sharing_policy: None,
        },
    ]
}

/// The request that creates one page documenting `parts`, with the optional members a row
/// names.
fn create_request(
    parts: &Value,
    display_content: Option<&Value>,
    try_it_state: Option<&str>,
) -> Value {
    let mut request = json!({ "restEndpointIdentifier": { "identifierParts": parts.clone() } });
    if let Some(content) = display_content {
        request["displayContent"] = content.clone();
    }
    if let Some(state) = try_it_state {
        request["tryItState"] = json!(state);
    }
    request
}

#[tokio::test]
async fn delete_product_rest_endpoint_page_removes_the_record_and_a_later_read_is_not_found() {
    // Every endpoint a row documents is created through the service that OWNS REST APIs: a
    // page may only document an endpoint the account has.
    let emulator = emulator_with_rest_apis();

    for row in rows() {
        let product_id = product(&emulator, row.case).await;
        let held = held_endpoint(&emulator, &row.parts).await;
        let page_id = create_page(
            &emulator,
            row.case,
            &product_id,
            create_request(
                &held.parts,
                row.display_content.as_ref(),
                row.try_it_state,
            ),
        )
        .await;

        let mut siblings = Vec::new();
        for sibling in &row.siblings {
            let sibling_held = held_endpoint(&emulator, sibling).await;
            siblings.push((
                sibling_held.parts.clone(),
                create_page(
                    &emulator,
                    row.case,
                    &product_id,
                    create_request(&sibling_held.parts, None, None),
                )
                .await,
            ));
        }

        // The other sub-resources of the same product, which a page delete must leave
        // where they are.
        let mut product_pages = Vec::new();
        for title in row.product_page_titles {
            let (status, created) = call(
                &emulator,
                "POST",
                &format!("{PRODUCTS}/{product_id}/productpages"),
                Some(json!({ "displayContent": { "title": title, "body": "a body" } })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {:?}: expected 201 from CreateProductPage, got {status} {created}",
                row.case
            );
            product_pages.push((*title, as_str(&created, "productPageId")));
        }
        if let Some(policy) = row.sharing_policy {
            let (status, body) = call(
                &emulator,
                "PUT",
                &format!("{PRODUCTS}/{product_id}/sharingpolicy"),
                Some(json!({ "policyDocument": policy })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected 200 from PutPortalProductSharingPolicy, got {status} {body}",
                row.case
            );
        }

        let (status, body) = delete_page(&emulator, &product_id, &page_id).await;
        assert_eq!(
            status, 204,
            "case {:?}: expected 204 from DeleteProductRestEndpointPage, got {status} {body}",
            row.case
        );
        assert_eq!(
            body,
            Value::Null,
            "case {:?}: expected an empty DeleteProductRestEndpointPage body, got {body}",
            row.case
        );

        // Read back through the implemented reader: the record is gone.
        let (status, body) = get_page(&emulator, &product_id, &page_id).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404 from GetProductRestEndpointPage after the delete, \
             got {status} {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {:?}: expected NotFoundException, got {body}",
            row.case
        );

        // A second delete asks for a state the store is already in, so it answers the same
        // empty 204 as the first and removes nothing again.
        let (status, body) = delete_page(&emulator, &product_id, &page_id).await;
        assert_eq!(
            status, 204,
            "case {:?}: expected 204 from the second DeleteProductRestEndpointPage, \
             got {status} {body}",
            row.case
        );
        assert_eq!(
            body,
            Value::Null,
            "case {:?}: expected an empty body from the second \
             DeleteProductRestEndpointPage, got {body}",
            row.case
        );

        // Every sibling page survives, still reporting the endpoint it documents.
        for (sibling_parts, sibling_id) in &siblings {
            let (status, fetched) = get_page(&emulator, &product_id, sibling_id).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected sibling page {sibling_id} to survive the delete, \
                 got {status} {fetched}",
                row.case
            );
            assert_eq!(
                fetched
                    .get("restEndpointIdentifier")
                    .and_then(|identifier| identifier.get("identifierParts")),
                Some(sibling_parts),
                "case {:?}: expected sibling page {sibling_id} to keep the endpoint it \
                 documents, got {fetched}",
                row.case
            );
        }

        // The product itself, its product pages and its sharing policy are untouched.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("{PRODUCTS}/{product_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the portal product to survive the delete, got {status} {fetched}",
            row.case
        );
        for (title, product_page_id) in &product_pages {
            let (status, fetched) = call(
                &emulator,
                "GET",
                &format!("{PRODUCTS}/{product_id}/productpages/{product_page_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected product page {product_page_id} to survive, got \
                 {status} {fetched}",
                row.case
            );
            assert_eq!(
                fetched
                    .get("displayContent")
                    .and_then(|content| content.get("title")),
                Some(&json!(title)),
                "case {:?}: expected product page {product_page_id} to keep its content, \
                 got {fetched}",
                row.case
            );
        }
        if let Some(policy) = row.sharing_policy {
            let (status, fetched) = call(
                &emulator,
                "GET",
                &format!("{PRODUCTS}/{product_id}/sharingpolicy"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected the sharing policy to survive, got {status} {fetched}",
                row.case
            );
            assert_eq!(
                as_str(&fetched, "policyDocument"),
                policy,
                "case {:?}: expected the stored sharing policy, got {fetched}",
                row.case
            );
        }

        // The product takes a page documenting the same endpoint again, and that page is a
        // NEW page: a freshly minted id, not the one the delete removed.
        let recreated = create_page(
            &emulator,
            row.case,
            &product_id,
            create_request(&held.parts, None, None),
        )
        .await;
        assert_minted_id(row.case, "ProductRestEndpointPageId", &recreated);
        assert_ne!(
            recreated, page_id,
            "case {:?}: expected a freshly minted ProductRestEndpointPageId for the \
             recreated page, got the deleted page's id {page_id}",
            row.case
        );
        let (status, fetched) = get_page(&emulator, &product_id, &recreated).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetProductRestEndpointPage for the recreated \
             page, got {status} {fetched}",
            row.case
        );
        // The recreated page carries the defaults of ITS create, not the deleted page's
        // overrides: the delete took the stored content with the record.
        assert_eq!(
            content_member(&fetched, "endpoint"),
            Some(held.invoke_url(REGION)),
            "case {:?}: expected the recreated page to report its own derived endpoint, \
             got {fetched}",
            row.case
        );
    }
}

/// A page is addressed by the account's product and its own minted id; the Region a
/// caller signs for names where derived values point, not which pages exist. So a page
/// created through a request signed for one Region is deleted through a request signed
/// for another, exactly as `GetProductRestEndpointPage` reads one.
#[tokio::test]
async fn a_page_is_deleted_whatever_region_the_request_is_signed_for() {
    let emulator = emulator_with_rest_apis();
    let held = held_endpoint(&emulator, &parts("regional02", "prod", "GET", "/orders")).await;

    for (created_in, deleted_in) in [
        ("eu-west-1", "us-east-1"),
        ("us-east-1", "ap-southeast-2"),
        ("ap-southeast-2", "ap-southeast-2"),
    ] {
        let case = format!("created in {created_in}, deleted from {deleted_in}");
        let (status, created_product) = call_in_region(
            &emulator,
            created_in,
            "POST",
            PRODUCTS,
            Some(json!({ "displayName": &case })),
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
            created_in,
            "POST",
            &collection,
            Some(create_request(&held.parts, None, None)),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateProductRestEndpointPage, got {status} \
             {created}"
        );
        let page_id = as_str(&created, "productRestEndpointPageId");

        let (status, body) = call_in_region(
            &emulator,
            deleted_in,
            "DELETE",
            &format!("{collection}/{page_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204 from DeleteProductRestEndpointPage, got {status} {body}"
        );

        let (status, body) = get_page(&emulator, &product_id, &page_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetProductRestEndpointPage after the delete, \
             got {status} {body}"
        );
    }
}

#[tokio::test]
async fn delete_product_rest_endpoint_page_refuses_only_an_unresolved_portal_product() {
    let emulator = emulator_with_rest_apis();
    let held = held_endpoint(&emulator, &parts("ownerpage1", "prod", "GET", "/orders")).await;

    let owner = product(&emulator, "Owner").await;
    // A second product that holds no page, so a page of the first addressed under it
    // resolves to nothing: a `ProductRestEndpointPageId` is minted per product, so it only
    // names a page of the product it was minted under.
    let other = product(&emulator, "Other owner").await;
    let page_id = create_page(
        &emulator,
        "the owning product's page",
        &owner,
        create_request(&held.parts, None, None),
    )
    .await;
    // The minted id is ten LOWERCASE alphanumerics (`__stringMin10Max30PatternAZ09`,
    // pattern `^[a-z0-9]+$`), so its uppercase form is an id nothing minted.
    let uppercase_page_id = page_id.to_uppercase();

    // A page id that resolves to nothing under a product the account HOLDS: the delete asks
    // for a state that product is already in, so it answers the empty 204.
    for (case, product_id, target_id) in [
        ("a page id nothing minted", owner.as_str(), "nosuchpage1"),
        (
            "the page id's uppercase form, which nothing minted",
            owner.as_str(),
            uppercase_page_id.as_str(),
        ),
        (
            "a page addressed under another product",
            other.as_str(),
            page_id.as_str(),
        ),
    ] {
        let (status, body) = delete_page(&emulator, product_id, target_id).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204 from DeleteProductRestEndpointPage, got {status} \
             {body}"
        );
        assert_eq!(
            body,
            Value::Null,
            "case {case:?}: expected an empty DeleteProductRestEndpointPage body, got {body}"
        );
    }

    // A PortalProductId nothing minted names no product to delete a page from at all.
    let (status, body) = delete_page(&emulator, "nosuchproduct", &page_id).await;
    assert_eq!(
        status, 404,
        "case \"a portal product nothing minted\": expected 404 from \
         DeleteProductRestEndpointPage, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "case \"a portal product nothing minted\": expected NotFoundException, got {body}"
    );
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        // The sentence `CreateProductRestEndpointPage` and
        // `DeletePortalProductSharingPolicy` already refuse an unresolved portal product
        // with — a product is an account-level resource, so the account the lookup was
        // made in and the id the request named are both in it — written out here so the
        // case asserts the rule.
        Some(
            format!(
                "Product not found in account {} with id nosuchproduct",
                engine::emulator_account_id()
            )
            .as_str()
        ),
        "case \"a portal product nothing minted\": expected the sentence this API refuses an \
         unresolved portal product with, got {body}"
    );

    // None of those calls removed the page the owning product holds.
    let (status, body) = get_page(&emulator, &owner, &page_id).await;
    assert_eq!(
        status, 200,
        "expected page {page_id} to survive every delete of another id, got {status} {body}"
    );
}
