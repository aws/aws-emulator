// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what `DeletePortalProduct` REMOVES.
//!
//! The rule these cases assert: **a portal product OWNS every resource addressed under it —
//! its product pages, its product REST endpoint pages and its sharing policy — so
//! `DELETE /v2/portalproducts/{PortalProductId}` takes all of them with the product, answers
//! the modelled empty `204` that states the length it has, and leaves every product the call
//! did not address holding exactly what it held, members and all.** Afterwards the product's
//! own reader reports the account-qualified product sentence, and each reader of something
//! that was under it reports that KIND's own unresolved-identifier sentence.
//! `smithy_query(query="operation",operation="DeletePortalProduct")` gives
//! `http {method: DELETE, uri: /v2/portalproducts/{PortalProductId}, code: 204}` over an
//! output of `Unit`, with `PortalProductId` a required `httpLabel`.
//!
//! The rows span what the rule generalises over: WHAT the deleted product holds when the
//! delete arrives — nothing, one product page, several product pages, one and several REST
//! endpoint pages, a sharing policy alone, and all three kinds at once — while a sibling
//! product carries the same furniture in every row, so each case tells a delete confined to
//! the addressed product apart from one that cleared the account.
//!
//! What this suite does NOT repeat: the idempotence of the delete over an id that resolves
//! to nothing (`absent_portal_product_delete_idempotence`), the IAM decision in front of it
//! (`portal_product_member_authorization`), and the per-operation not-found wording
//! (`not_found_sentence_per_addressing_operation`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{
    as_str, call, call_reading_headers, emulator_with_rest_apis, error_code, rest_endpoint,
};
use crate::product_page_lifecycle::pages_of as product_pages_of;
use crate::product_rest_endpoint_page_lifecycle::{
    pages_of as rest_endpoint_pages_of, product, PRODUCTS,
};

/// The error code every reader of an identifier that resolves to nothing carries, from the
/// model's own 404 error.
const NOT_FOUND: &str = "NotFoundException";

/// The sentence a `ProductPage` that resolves to nothing is reported with
/// (`not_found_sentence_per_addressing_operation` owns the full table of these).
const PRODUCT_PAGE_NOT_FOUND: &str = "ProductPage not found";

/// The sentence a `ProductRestEndpointPage` that resolves to nothing is reported with.
const REST_ENDPOINT_PAGE_NOT_FOUND: &str = "ProductRestEndpointPage not found";

/// The policy document a product with no sharing policy of its own reports: a product shared
/// with nobody grants nothing.
const EMPTY_POLICY: &str = "{}";

/// AWS's sentence for a `PortalProductId` the account holds no product under: a product is an
/// account-level resource, so the account the lookup was made in is named beside the id.
/// Written out here rather than taken from the service, so the cases assert the rule.
fn product_not_found(portal_product_id: &str) -> String {
    format!(
        "Product not found in account {} with id {portal_product_id}",
        engine::emulator_account_id()
    )
}

/// The sharing-policy member of one product (`PUT|GET|DELETE
/// /v2/portalproducts/{PortalProductId}/sharingpolicy`).
fn sharing_policy_of(portal_product_id: &str) -> String {
    format!("{PRODUCTS}/{portal_product_id}/sharingpolicy")
}

/// A sharing-policy document naming `principal`, so each row's product carries a document of
/// its own and a survivor's policy cannot be mistaken for another product's.
fn policy_document(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": principal },
            "Action": "apigateway:GET"
        }]
    })
    .to_string()
}

/// One case: what the addressed product (and the sibling beside it) holds when the delete
/// arrives.
struct Row {
    case: &'static str,
    product_pages: usize,
    rest_endpoint_pages: usize,
    sharing_policy: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a product holding nothing at all",
            product_pages: 0,
            rest_endpoint_pages: 0,
            sharing_policy: false,
        },
        Row {
            case: "a product holding one product page",
            product_pages: 1,
            rest_endpoint_pages: 0,
            sharing_policy: false,
        },
        Row {
            case: "a product holding three product pages",
            product_pages: 3,
            rest_endpoint_pages: 0,
            sharing_policy: false,
        },
        Row {
            case: "a product holding one REST endpoint page",
            product_pages: 0,
            rest_endpoint_pages: 1,
            sharing_policy: false,
        },
        Row {
            case: "a product holding two REST endpoint pages",
            product_pages: 0,
            rest_endpoint_pages: 2,
            sharing_policy: false,
        },
        Row {
            case: "a product holding a sharing policy and no pages",
            product_pages: 0,
            rest_endpoint_pages: 0,
            sharing_policy: true,
        },
        Row {
            case: "a product holding pages of both kinds and a sharing policy",
            product_pages: 2,
            rest_endpoint_pages: 2,
            sharing_policy: true,
        },
    ]
}

/// What one product of a row holds: its id and the ids of the members created under it.
struct Furnished {
    portal_product_id: String,
    product_pages: Vec<String>,
    rest_endpoint_pages: Vec<String>,
    /// The document this product's sharing policy was written with, where the row wrote one.
    policy: Option<String>,
}

/// Create one product and the members the row asks for under it.
async fn furnish(
    emulator: &MultiServiceRouter,
    case: &str,
    label: &str,
    row: &Row,
    rest_api_id: &str,
) -> Furnished {
    let portal_product_id = product(emulator, &format!("{case} ({label})")).await;

    let mut product_pages = Vec::new();
    for index in 0..row.product_pages {
        let (status, created) = call(
            emulator,
            "POST",
            &product_pages_of(&portal_product_id),
            Some(json!({
                "displayContent": {
                    "title": format!("{label} page {index}"),
                    "body": "The body of a page this case created."
                }
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?} ({label}): expected 201 from CreateProductPage, got {status} {created}"
        );
        product_pages.push(as_str(&created, "productPageId"));
    }

    let mut rest_endpoint_pages = Vec::new();
    for index in 0..row.rest_endpoint_pages {
        let (status, created) = call(
            emulator,
            "POST",
            &rest_endpoint_pages_of(&portal_product_id),
            Some(json!({
                "restEndpointIdentifier": {
                    "identifierParts": {
                        "restApiId": rest_api_id,
                        "stage": "prod",
                        "method": "GET",
                        "path": format!("/{label}/{index}")
                    }
                }
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?} ({label}): expected 201 from CreateProductRestEndpointPage, got \
             {status} {created}"
        );
        rest_endpoint_pages.push(as_str(&created, "productRestEndpointPageId"));
    }

    let policy = if row.sharing_policy {
        // A document of this product's own, so a survivor's policy cannot be mistaken for
        // the policy of the product the row deleted.
        let document = policy_document(&format!(
            "arn:aws:iam::{}:role/{label}",
            engine::emulator_account_id()
        ));
        let (status, written) = call(
            emulator,
            "PUT",
            &sharing_policy_of(&portal_product_id),
            Some(json!({ "policyDocument": document })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?} ({label}): expected 200 from PutPortalProductSharingPolicy, got \
             {status} {written}"
        );
        Some(document)
    } else {
        None
    };

    Furnished {
        portal_product_id,
        product_pages,
        rest_endpoint_pages,
        policy,
    }
}

/// The identifiers `ListPortalProducts` reports for the account.
async fn listed_products(emulator: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, listed) = call(emulator, "GET", PRODUCTS, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListPortalProducts, got {status} {listed}"
    );
    listed["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| as_str(item, "portalProductId"))
        .collect()
}

/// Assert `uri` answers 404 `NotFoundException` with `expected_message`.
async fn assert_not_found(
    emulator: &MultiServiceRouter,
    case: &str,
    what: &str,
    uri: &str,
    expected_message: &str,
) {
    let (status, body) = call(emulator, "GET", uri, None).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404 reading {what} at {uri}, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        NOT_FOUND,
        "case {case:?}: expected {NOT_FOUND} reading {what} at {uri}, got {body}"
    );
    assert_eq!(
        as_str(&body, "message"),
        expected_message,
        "case {case:?}: expected the sentence {expected_message:?} reading {what} at {uri}, got \
         {body}"
    );
}

/// Assert everything `held` holds is still exactly as it was: the product, each page under it
/// and the sharing-policy document it carries.
async fn assert_intact(emulator: &MultiServiceRouter, case: &str, held: &Furnished) {
    let id = &held.portal_product_id;
    let (status, read) = call(emulator, "GET", &format!("{PRODUCTS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the untouched product {id} to still read back, got {status} \
         {read}"
    );
    assert_eq!(
        as_str(&read, "portalProductId"),
        *id,
        "case {case:?}: expected the read to report the product addressed, got {read}"
    );

    for page in &held.product_pages {
        let (status, read) = call(
            emulator,
            "GET",
            &format!("{}/{page}", product_pages_of(id)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected product page {page} of the untouched {id} to survive, got \
             {status} {read}"
        );
        assert_eq!(
            as_str(&read, "productPageId"),
            *page,
            "case {case:?}: expected the surviving page to report its own id, got {read}"
        );
    }

    for page in &held.rest_endpoint_pages {
        let (status, read) = call(
            emulator,
            "GET",
            &format!("{}/{page}", rest_endpoint_pages_of(id)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected REST endpoint page {page} of the untouched {id} to \
             survive, got {status} {read}"
        );
        assert_eq!(
            as_str(&read, "productRestEndpointPageId"),
            *page,
            "case {case:?}: expected the surviving page to report its own id, got {read}"
        );
    }

    let (status, read) = call(emulator, "GET", &sharing_policy_of(id), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the sharing policy of the untouched {id} to still read back, \
         got {status} {read}"
    );
    let expected_policy = held.policy.clone().unwrap_or_else(|| EMPTY_POLICY.to_string());
    assert_eq!(
        as_str(&read, "policyDocument"),
        expected_policy,
        "case {case:?}: expected the untouched {id} to keep the document {expected_policy:?}, \
         got {read}"
    );
}

/// RULE: the delete of one portal product removes the product and every resource addressed
/// under it, answers the modelled empty 204, and touches no other product.
#[tokio::test]
async fn deleting_a_portal_product_removes_everything_it_owns_and_nothing_else() {
    // A REST endpoint page documents an endpoint the account HOLDS, so the emulator also
    // holds the service that owns REST APIs.
    let emulator = emulator_with_rest_apis();
    let rest_api_id = rest_endpoint(&emulator, "product-deletion-rest-api", "prod").await;

    for row in rows() {
        let case = row.case;
        let deleted = furnish(&emulator, case, "deleted", &row, &rest_api_id).await;
        let kept = furnish(&emulator, case, "kept", &row, &rest_api_id).await;

        let uri = format!("{PRODUCTS}/{}", deleted.portal_product_id);
        let (status, headers, body) = call_reading_headers(&emulator, "DELETE", &uri, None).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected the modelled 204 from DeletePortalProduct, got {status} \
             {body}"
        );
        assert_eq!(
            body,
            Value::Null,
            "case {case:?}: expected an answer carrying no body, got {body}"
        );
        assert_eq!(
            headers
                .get("content-length")
                .and_then(|value| value.to_str().ok()),
            Some("0"),
            "case {case:?}: expected the empty answer to state content-length 0, got {:?}",
            headers.get("content-length")
        );

        // The product itself is gone, and the account-level sentence names the account the
        // lookup was made in beside the id the request supplied.
        assert_not_found(
            &emulator,
            case,
            "the deleted product",
            &uri,
            &product_not_found(&deleted.portal_product_id),
        )
        .await;
        // Its sharing policy went with it: no product, no policy of it to report.
        assert_not_found(
            &emulator,
            case,
            "the sharing policy of the deleted product",
            &sharing_policy_of(&deleted.portal_product_id),
            &product_not_found(&deleted.portal_product_id),
        )
        .await;
        // So did every page it owned, each reported as that kind of page resolving to nothing.
        for page in &deleted.product_pages {
            assert_not_found(
                &emulator,
                case,
                "a product page of the deleted product",
                &format!(
                    "{}/{page}",
                    product_pages_of(&deleted.portal_product_id)
                ),
                PRODUCT_PAGE_NOT_FOUND,
            )
            .await;
        }
        for page in &deleted.rest_endpoint_pages {
            assert_not_found(
                &emulator,
                case,
                "a REST endpoint page of the deleted product",
                &format!(
                    "{}/{page}",
                    rest_endpoint_pages_of(&deleted.portal_product_id)
                ),
                REST_ENDPOINT_PAGE_NOT_FOUND,
            )
            .await;
        }

        // The account's listing drops the deleted product and keeps the sibling.
        let reported = listed_products(&emulator, case).await;
        assert!(
            !reported.contains(&deleted.portal_product_id),
            "case {case:?}: expected the deleted {} to be gone from the listing, got {reported:?}",
            deleted.portal_product_id
        );
        assert!(
            reported.contains(&kept.portal_product_id),
            "case {case:?}: expected the listing to still report {}, got {reported:?}",
            kept.portal_product_id
        );

        // And the sibling, furnished exactly like the deleted product, is untouched.
        assert_intact(&emulator, case, &kept).await;
    }
}

/// RULE: a portal product owns its pages, not the resources they DOCUMENT — so the REST API a
/// deleted product's REST endpoint page named is still held by the service that owns it.
#[tokio::test]
async fn deleting_a_portal_product_leaves_the_rest_api_its_page_documented_alone() {
    let emulator = emulator_with_rest_apis();
    let rest_api_id = rest_endpoint(&emulator, "documented-rest-api", "prod").await;
    let row = Row {
        case: "a product whose page documents a held REST endpoint",
        product_pages: 0,
        rest_endpoint_pages: 1,
        sharing_policy: false,
    };
    let held = furnish(&emulator, row.case, "deleted", &row, &rest_api_id).await;

    let (status, body) = call(
        &emulator,
        "DELETE",
        &format!("{PRODUCTS}/{}", held.portal_product_id),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "case {:?}: expected the modelled 204 from DeletePortalProduct, got {status} {body}",
        row.case
    );

    let (status, read) = call(&emulator, "GET", &format!("/restapis/{rest_api_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the REST API {rest_api_id} to survive the product delete, got \
         {status} {read}",
        row.case
    );
    assert_eq!(
        as_str(&read, "id"),
        rest_api_id,
        "case {:?}: expected the surviving REST API to report its own id, got {read}",
        row.case
    );
}
