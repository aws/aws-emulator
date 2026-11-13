// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `DeleteProductPage`.
//!
//! The rules these cases assert:
//!
//! * **A product page its portal product holds is removed**: the delete answers 204 with
//!   an empty body, `GetProductPage` then reports 404 `NotFoundException`, and a second
//!   delete of the same id answers the same empty 204 — the delete is idempotent, so a
//!   page that is already gone is already in the state the call asks for. A page created
//!   afterwards is a NEW page, with a freshly minted id and the content of ITS create.
//! * **The delete reaches exactly one page**: every sibling product page of the same
//!   product survives and still reports its own content, and so do the product itself,
//!   its product REST endpoint pages and its sharing policy — a page is a child of its
//!   product, not the other way round.
//! * **A page is addressed by its product and its own minted id, not by the Region the
//!   caller signs for**: a page created through a request signed for one Region is deleted
//!   through a request signed for another, the way `GetProductPage` reads one.
//! * **The PRODUCT is the resource that has to exist**: a `PortalProductId` nothing minted
//!   is a 404 `NotFoundException` carrying the sentence this API refuses an unresolved
//!   portal product with, and removes nothing; a `ProductPageId` that resolves to nothing
//!   UNDER a product the account holds is a 204 that removes nothing — a page id nothing
//!   minted (short and at the modelled 30-char maximum), the id's uppercase form, and a
//!   page of one product addressed under another.
//!
//! The rows vary what the rules generalise over: the deleted page's title and body
//! (shortest accepted, multi-line, non-ASCII, both members at their modelled maxima), how
//! many sibling pages the product holds, and which other sub-resources the product
//! carries. The recorded lifecycle that filed the issue — one page created under one
//! product and then deleted — is ONE row of the first table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{
    as_str, assert_minted_id, call, call_in_region, emulator, emulator_with_rest_apis, error_code,
    rest_endpoint,
};
use crate::product_page_lifecycle::{pages_of, BODY_MAX_LENGTH, TITLE_MAX_LENGTH};
use crate::product_rest_endpoint_page_lifecycle::{product, PRODUCTS};

/// The sentence this API refuses a portal product identifier that resolves to nothing with,
/// the one `CreateProductPage` and `DeletePortalProductSharingPolicy` already answer: a
/// product is an account-level resource, so the account the lookup was made in and the id
/// the request named are both in it. Written out here rather than taken from the service,
/// so the table asserts the rule.
fn product_not_found_message(portal_product_id: &str) -> String {
    format!(
        "Product not found in account {} with id {portal_product_id}",
        engine::emulator_account_id()
    )
}

/// Create one product page under `portal_product_id` and answer its `ProductPageId`.
async fn create_page(
    emulator: &MultiServiceRouter,
    case: &str,
    portal_product_id: &str,
    title: &str,
    body: &str,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &pages_of(portal_product_id),
        Some(json!({ "displayContent": { "title": title, "body": body } })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateProductPage, got {status} {created}"
    );
    as_str(&created, "productPageId")
}

/// `(status, body)` of `GetProductPage`.
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

/// `(status, body)` of `DeleteProductPage`.
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

/// The `displayContent.title` a response reports, if any.
fn title_of(document: &Value) -> Option<&str> {
    document
        .get("displayContent")
        .and_then(|content| content.get("title"))
        .and_then(Value::as_str)
}

/// One row of the "the delete goes through" table: the deleted page's content, the
/// sibling pages created beside it, and the other sub-resources its product carries.
struct Row {
    case: &'static str,
    /// The deleted page's `DisplayContent$Title`.
    title: String,
    /// The deleted page's `DisplayContent$Body`.
    body: String,
    /// The titles of the sibling product pages, none of which this row deletes.
    sibling_titles: Vec<String>,
    /// `IdentifierParts$RestApiId`s of the product REST endpoint pages the product also
    /// holds, which a product-page delete must not touch.
    /// The REST APIs, one per product REST endpoint page this product also holds, named as
    /// they are created through the service that OWNS REST APIs — a page documents an
    /// endpoint the account HAS.
    rest_endpoint_pages: &'static [&'static str],
    /// The sharing policy the product carries, if any.
    sharing_policy: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the only page of its product, with a plain title and body",
            title: "Getting started".to_string(),
            body: "Call the orders endpoint to place an order.".to_string(),
            sibling_titles: Vec::new(),
            rest_endpoint_pages: &[],
            sharing_policy: None,
        },
        Row {
            case: "both members at the shortest length the model accepts (1), beside one \
                   sibling",
            title: "t".to_string(),
            body: "b".to_string(),
            sibling_titles: vec!["Reference".to_string()],
            rest_endpoint_pages: &[],
            sharing_policy: None,
        },
        Row {
            case: "a multi-line markdown body, beside several siblings",
            title: "Reference".to_string(),
            body: "# Orders\n\n- `GET /orders`\n- `POST /orders`\n\nSee the guide.\n".to_string(),
            sibling_titles: vec![
                "Overview".to_string(),
                "Pricing".to_string(),
                "Support".to_string(),
            ],
            rest_endpoint_pages: &[],
            sharing_policy: None,
        },
        Row {
            case: "a non-ASCII title and body, on a product that also holds REST endpoint \
                   pages and a sharing policy",
            title: "Übersicht — 概要".to_string(),
            body: "Grüße! 価格は無料です。\u{1f680}".to_string(),
            sibling_titles: vec!["Overview".to_string()],
            rest_endpoint_pages: &["deletion-rest-one", "deletion-rest-two"],
            sharing_policy: Some(
                "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                 \"Principal\":{\"AWS\":\"*\"},\"Action\":\"apigateway:GET\"}]}",
            ),
        },
        Row {
            // The published bounds of `DisplayContent` (len 1..255 and len 1..32768): a
            // page at the largest content the model accepts is deleted like any other.
            case: "both members at their modelled maxima",
            title: "t".repeat(TITLE_MAX_LENGTH),
            body: "b".repeat(BODY_MAX_LENGTH),
            sibling_titles: vec!["Getting started".to_string()],
            rest_endpoint_pages: &[],
            sharing_policy: None,
        },
    ]
}

#[tokio::test]
async fn delete_product_page_removes_the_record_and_a_later_read_is_not_found() {
    let emulator = emulator_with_rest_apis();

    for row in rows() {
        let product_id = product(&emulator, row.case).await;
        let page_id = create_page(&emulator, row.case, &product_id, &row.title, &row.body).await;

        let mut siblings = Vec::new();
        for title in &row.sibling_titles {
            siblings.push((
                title.clone(),
                create_page(&emulator, row.case, &product_id, title, "a sibling body").await,
            ));
        }

        // The other sub-resources of the same product, which a product-page delete must
        // leave where they are.
        let mut rest_endpoint_pages = Vec::new();
        for rest_api_name in row.rest_endpoint_pages {
            let rest_api_id = rest_endpoint(&emulator, rest_api_name, "prod").await;
            let (status, created) = call(
                &emulator,
                "POST",
                &format!("{PRODUCTS}/{product_id}/productrestendpointpages"),
                Some(json!({
                    "restEndpointIdentifier": {
                        "identifierParts": {
                            "restApiId": rest_api_id,
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
                "case {:?}: expected 201 from CreateProductRestEndpointPage, got {status} \
                 {created}",
                row.case
            );
            rest_endpoint_pages.push(as_str(&created, "productRestEndpointPageId"));
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
            "case {:?}: expected 204 from DeleteProductPage, got {status} {body}",
            row.case
        );
        assert_eq!(
            body,
            Value::Null,
            "case {:?}: expected an empty DeleteProductPage body, got {body}",
            row.case
        );

        // Read back through the implemented reader: the record is gone.
        let (status, body) = get_page(&emulator, &product_id, &page_id).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404 from GetProductPage after the delete, got {status} {body}",
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
            "case {:?}: expected 204 from the second DeleteProductPage, got {status} {body}",
            row.case
        );
        assert_eq!(
            body,
            Value::Null,
            "case {:?}: expected an empty body from the second DeleteProductPage, got {body}",
            row.case
        );

        // Every sibling page survives, still reporting the content it was created with.
        for (sibling_title, sibling_id) in &siblings {
            let (status, fetched) = get_page(&emulator, &product_id, sibling_id).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected sibling page {sibling_id} to survive the delete, got \
                 {status} {fetched}",
                row.case
            );
            assert_eq!(
                title_of(&fetched),
                Some(sibling_title.as_str()),
                "case {:?}: expected sibling page {sibling_id} to keep its content, got \
                 {fetched}",
                row.case
            );
        }

        // The product itself, its REST endpoint pages and its sharing policy are untouched.
        let (status, fetched) =
            call(&emulator, "GET", &format!("{PRODUCTS}/{product_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the portal product to survive the delete, got {status} \
             {fetched}",
            row.case
        );
        for rest_page_id in &rest_endpoint_pages {
            let (status, fetched) = call(
                &emulator,
                "GET",
                &format!("{PRODUCTS}/{product_id}/productrestendpointpages/{rest_page_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected REST endpoint page {rest_page_id} to survive, got \
                 {status} {fetched}",
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

        // The product takes a page with the same content again, and that page is a NEW
        // page: a freshly minted id, carrying the content of ITS create.
        let recreated = create_page(
            &emulator,
            row.case,
            &product_id,
            "Recreated",
            "A body of its own",
        )
        .await;
        assert_minted_id(row.case, "productPageId", &recreated);
        assert_ne!(
            recreated, page_id,
            "case {:?}: expected a freshly minted productPageId for the recreated page, \
             got the deleted page's id {page_id}",
            row.case
        );
        let (status, fetched) = get_page(&emulator, &product_id, &recreated).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetProductPage for the recreated page, got \
             {status} {fetched}",
            row.case
        );
        assert_eq!(
            title_of(&fetched),
            Some("Recreated"),
            "case {:?}: expected the recreated page to report its own content, got {fetched}",
            row.case
        );
    }
}

/// A product page is addressed by the account's product and its own minted id; the Region
/// a caller signs for names where derived values point, not which pages exist. So a page
/// created through a request signed for one Region is deleted through a request signed for
/// another, exactly as `GetProductPage` reads one.
#[tokio::test]
async fn a_product_page_is_deleted_whatever_region_the_request_is_signed_for() {
    let emulator = emulator();

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
            Some(json!({ "displayContent": { "title": "Regional", "body": "Regional body" } })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateProductPage, got {status} {created}"
        );
        let page_id = as_str(&created, "productPageId");

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
            "case {case:?}: expected 204 from DeleteProductPage, got {status} {body}"
        );

        let (status, body) = get_page(&emulator, &product_id, &page_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetProductPage after the delete, got {status} \
             {body}"
        );
    }
}

#[tokio::test]
async fn delete_product_page_refuses_only_an_unresolved_portal_product() {
    let emulator = emulator();

    let owner = product(&emulator, "Owner").await;
    // A second product that holds no page, so a page of the first addressed under it
    // resolves to nothing: a `ProductPageId` is minted per product, so it only names a
    // page of the product it was minted under.
    let other = product(&emulator, "Other owner").await;
    let page_id = create_page(
        &emulator,
        "the owning product's page",
        &owner,
        "Only page",
        "Only body",
    )
    .await;
    // The minted id is ten LOWERCASE alphanumerics (`__stringMin10Max30PatternAZ09`,
    // pattern `^[a-z0-9]+$`), so its uppercase form is an id nothing minted.
    let uppercase_page_id = page_id.to_uppercase();
    // An id at the modelled 30-character maximum, which nothing minted either.
    let longest_page_id = "z".repeat(30);

    // A page id that resolves to nothing under a product the account HOLDS: the delete
    // asks for a state that product is already in, so it answers the empty 204.
    for (case, product_id, target_id) in [
        ("a page id nothing minted", owner.as_str(), "nosuchpage1"),
        (
            "an id at the modelled maximum length, which nothing minted",
            owner.as_str(),
            longest_page_id.as_str(),
        ),
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
            "case {case:?}: expected 204 from DeleteProductPage, got {status} {body}"
        );
        assert_eq!(
            body,
            Value::Null,
            "case {case:?}: expected an empty DeleteProductPage body, got {body}"
        );
    }

    // A PortalProductId nothing minted names no product to delete a page from at all.
    let (status, body) = delete_page(&emulator, "nosuchproduct", &page_id).await;
    assert_eq!(
        status, 404,
        "case \"a portal product nothing minted\": expected 404 from DeleteProductPage, got \
         {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "case \"a portal product nothing minted\": expected NotFoundException, got {body}"
    );
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(product_not_found_message("nosuchproduct").as_str()),
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
