// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the IDEMPOTENCE of this service's portal-product page
//! deletes.
//!
//! The rule these cases assert: **deleting a page of a portal product the account holds
//! answers the modelled empty `204` whether or not that page is there to remove — the call
//! asks for a state (no page under this id), and a product already in that state is
//! answered the same way as one the call takes a page out of; only a `PortalProductId` the
//! account holds no product under is refused `404 NotFoundException`, naming the identifier
//! that resolved to nothing.** Both page kinds the model binds under a portal product are
//! covered: `DeleteProductPage`
//! (`DELETE /v2/portalproducts/{PortalProductId}/productpages/{ProductPageId}`, `code: 204`
//! over `Unit`) and `DeleteProductRestEndpointPage`
//! (`.../productrestendpointpages/{ProductRestEndpointPageId}`, likewise) —
//! `smithy_query(query="operation",operation="DeleteProductPage,DeleteProductRestEndpointPage")`.
//!
//! The rows span what the rule generalises over: the page kind, how the addressed id came
//! to resolve to nothing (a page this test deleted, an id nothing ever minted, the minted
//! id's uppercase form, an id at the modelled 30-character maximum, a page of ANOTHER
//! product), how many times the same delete is repeated (twice and three times), and how
//! many sibling pages the product holds while it happens. The recorded step that filed the
//! issue — create a page, delete it, then issue the identical delete again — is the first
//! row.
//!
//! Each case also asserts the delete that removed nothing CHANGED nothing: every sibling
//! page still reads back, and the product itself still does.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator_with_rest_apis, error_code, rest_endpoint};
use crate::product_page_lifecycle::pages_of as product_pages_of;
use crate::product_rest_endpoint_page_lifecycle::{
    pages_of as rest_endpoint_pages_of, product, PRODUCTS,
};

/// The upper bound of `__stringMin10Max30PatternAZ09`, the shape both page identifiers
/// target (`length: 10..30`, pattern `^[a-z0-9]+$`).
const MAX_PAGE_ID_LENGTH: usize = 30;

/// Which of the two page kinds a portal product holds a row addresses.
#[derive(Clone, Copy, Debug)]
enum Kind {
    ProductPage,
    RestEndpointPage,
}

impl Kind {
    /// The collection URI this kind's pages of `portal_product_id` live under.
    fn collection(self, portal_product_id: &str) -> String {
        match self {
            Self::ProductPage => product_pages_of(portal_product_id),
            Self::RestEndpointPage => rest_endpoint_pages_of(portal_product_id),
        }
    }

    /// The `Create…` body that mints one page of this kind, distinguished by `label`. A
    /// REST endpoint page documents an endpoint the ACCOUNT has, so `rest_api_id` is the id
    /// the service that owns REST APIs minted for this suite's API.
    fn create_body(self, label: &str, rest_api_id: &str) -> Value {
        match self {
            Self::ProductPage => json!({
                "displayContent": { "title": label, "body": format!("The body of {label}") }
            }),
            Self::RestEndpointPage => json!({
                "restEndpointIdentifier": {
                    "identifierParts": {
                        "restApiId": rest_api_id,
                        "stage": "prod",
                        "method": "GET",
                        "path": format!("/{label}")
                    }
                }
            }),
        }
    }

    /// The member of the create response carrying the minted identifier.
    fn id_member(self) -> &'static str {
        match self {
            Self::ProductPage => "productPageId",
            Self::RestEndpointPage => "productRestEndpointPageId",
        }
    }
}

/// How a row's addressed identifier comes to resolve to no page.
#[derive(Clone, Copy, Debug)]
enum Target {
    /// A page this row created and then deleted.
    DeletedByThisRow,
    /// An identifier nothing ever minted.
    NeverMinted,
    /// The minted id's uppercase form — the mint is lowercase, so this names no page.
    UppercaseOfMinted,
    /// An identifier at the modelled maximum length, which nothing minted.
    AtTheMaximumLength,
    /// The row's page, addressed under ANOTHER product of the same account.
    UnderAnotherProduct,
}

/// One case: a page kind, how its addressed id resolves to nothing, how many times the
/// delete is repeated, and how many sibling pages the product holds.
struct Row {
    case: &'static str,
    kind: Kind,
    target: Target,
    deletes: usize,
    siblings: usize,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a product page deleted and then deleted again",
            kind: Kind::ProductPage,
            target: Target::DeletedByThisRow,
            deletes: 2,
            siblings: 0,
        },
        Row {
            case: "a product page deleted three times, beside two siblings",
            kind: Kind::ProductPage,
            target: Target::DeletedByThisRow,
            deletes: 3,
            siblings: 2,
        },
        Row {
            case: "a product page id nothing ever minted",
            kind: Kind::ProductPage,
            target: Target::NeverMinted,
            deletes: 1,
            siblings: 1,
        },
        Row {
            case: "the uppercase form of a minted product page id",
            kind: Kind::ProductPage,
            target: Target::UppercaseOfMinted,
            deletes: 2,
            siblings: 0,
        },
        Row {
            case: "a product page id at the modelled maximum length",
            kind: Kind::ProductPage,
            target: Target::AtTheMaximumLength,
            deletes: 1,
            siblings: 0,
        },
        Row {
            case: "a product page of one product addressed under another",
            kind: Kind::ProductPage,
            target: Target::UnderAnotherProduct,
            deletes: 1,
            siblings: 0,
        },
        Row {
            case: "a REST endpoint page deleted and then deleted again",
            kind: Kind::RestEndpointPage,
            target: Target::DeletedByThisRow,
            deletes: 2,
            siblings: 0,
        },
        Row {
            case: "a REST endpoint page deleted three times, beside one sibling",
            kind: Kind::RestEndpointPage,
            target: Target::DeletedByThisRow,
            deletes: 3,
            siblings: 1,
        },
        Row {
            case: "a REST endpoint page id nothing ever minted",
            kind: Kind::RestEndpointPage,
            target: Target::NeverMinted,
            deletes: 2,
            siblings: 1,
        },
        Row {
            case: "a REST endpoint page id at the modelled maximum length",
            kind: Kind::RestEndpointPage,
            target: Target::AtTheMaximumLength,
            deletes: 1,
            siblings: 0,
        },
        Row {
            case: "a REST endpoint page of one product addressed under another",
            kind: Kind::RestEndpointPage,
            target: Target::UnderAnotherProduct,
            deletes: 1,
            siblings: 0,
        },
    ]
}

/// Create one page of `kind` under `portal_product_id` and answer its minted id.
async fn create_page(
    emulator: &MultiServiceRouter,
    case: &str,
    kind: Kind,
    portal_product_id: &str,
    label: &str,
    rest_api_id: &str,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &kind.collection(portal_product_id),
        Some(kind.create_body(label, rest_api_id)),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from the create of a {kind:?}, got {status} {created}"
    );
    as_str(&created, kind.id_member())
}

#[tokio::test]
async fn deleting_a_page_that_is_already_gone_answers_the_empty_204() {
    let emulator = emulator_with_rest_apis();
    // The one REST endpoint every REST endpoint page below documents, created through the
    // service that OWNS REST APIs.
    let rest_api_id = rest_endpoint(&emulator, "absent-page-rest", "prod").await;

    for row in rows() {
        let owner = product(&emulator, row.case).await;
        // A second product of the same account, which holds no page of its own: a page id
        // is minted per product, so the owner's page names nothing under this one.
        let other = product(&emulator, "another product").await;

        let minted =
            create_page(&emulator, row.case, row.kind, &owner, "target", &rest_api_id).await;
        let mut siblings = Vec::new();
        for index in 0..row.siblings {
            siblings.push(
                create_page(
                    &emulator,
                    row.case,
                    row.kind,
                    &owner,
                    &format!("sibling{index}"),
                    &rest_api_id,
                )
                .await,
            );
        }

        let (product_id, page_id) = match row.target {
            Target::DeletedByThisRow | Target::NeverMinted | Target::AtTheMaximumLength => {
                (owner.clone(), minted.clone())
            }
            Target::UppercaseOfMinted => (owner.clone(), minted.to_uppercase()),
            Target::UnderAnotherProduct => (other.clone(), minted.clone()),
        };
        let page_id = match row.target {
            Target::NeverMinted => "nosuchpage1".to_string(),
            Target::AtTheMaximumLength => "z".repeat(MAX_PAGE_ID_LENGTH),
            _ => page_id,
        };
        let uri = format!("{}/{page_id}", row.kind.collection(&product_id));

        // The row's first delete may be the one that removes the page; every delete after it
        // addresses a page that is already gone. Either way each answers the empty 204.
        for attempt in 1..=row.deletes {
            let (status, body) = call(&emulator, "DELETE", &uri, None).await;
            assert_eq!(
                status, 204,
                "case {:?}: expected 204 from delete attempt {attempt} of {uri}, got {status} \
                 {body}",
                row.case
            );
            assert_eq!(
                body,
                Value::Null,
                "case {:?}: expected an empty body from delete attempt {attempt} of {uri}, \
                 got {body}",
                row.case
            );
        }

        // A delete that removed nothing changed nothing: a page the row never addressed
        // still reads back, and so does the product itself.
        let survivors: Vec<String> = match row.target {
            Target::DeletedByThisRow => siblings.clone(),
            // These rows addressed an id that names no page, so the minted page survives too.
            _ => siblings.iter().cloned().chain([minted.clone()]).collect(),
        };
        for survivor in &survivors {
            let (status, fetched) = call(
                &emulator,
                "GET",
                &format!("{}/{survivor}", row.kind.collection(&owner)),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected page {survivor} to survive the delete of {uri}, got \
                 {status} {fetched}",
                row.case
            );
        }
        let (status, fetched) = call(&emulator, "GET", &format!("{PRODUCTS}/{owner}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the portal product to survive, got {status} {fetched}",
            row.case
        );

        // And the page the row deleted (where it deleted one) is really gone.
        if matches!(row.target, Target::DeletedByThisRow) {
            let (status, fetched) = call(
                &emulator,
                "GET",
                &format!("{}/{minted}", row.kind.collection(&owner)),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {:?}: expected the deleted page to be gone, got {status} {fetched}",
                row.case
            );
        }
    }
}

/// The PRODUCT is the resource that has to exist: a `PortalProductId` the account holds no
/// product under names nothing to delete a page from, for either page kind, whatever the
/// page identifier looks like.
#[tokio::test]
async fn deleting_a_page_of_a_product_nothing_minted_is_refused() {
    let emulator = emulator_with_rest_apis();
    let rest_api_id = rest_endpoint(&emulator, "absent-product-rest", "prod").await;

    // The sentence this API refuses an unresolved portal product with, the one
    // `CreateProductPage` and `DeletePortalProductSharingPolicy` already answer: a
    // product is an account-level resource, so the account the lookup was made in and
    // the id the request named are both in it. Written out here rather than taken from
    // the service, so the table asserts the rule.
    let product_not_found_message =
        format!("Product not found in account {} with id nosuchproduct", engine::emulator_account_id());

    let owner = product(&emulator, "the holder of a real page").await;

    for kind in [Kind::ProductPage, Kind::RestEndpointPage] {
        let minted = create_page(
            &emulator,
            "the holder's page",
            kind,
            &owner,
            "target",
            &rest_api_id,
        )
        .await;
        for page_id in [
            minted.clone(),
            "nosuchpage1".to_string(),
            "z".repeat(MAX_PAGE_ID_LENGTH),
        ] {
            let case = format!("{kind:?} {page_id} under a product nothing minted");
            let uri = format!("{}/{page_id}", kind.collection("nosuchproduct"));
            let (status, body) = call(&emulator, "DELETE", &uri, None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 from {uri}, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "NotFoundException",
                "case {case:?}: expected NotFoundException, got {body}"
            );
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(product_not_found_message.as_str()),
                "case {case:?}: expected the sentence this API refuses an unresolved portal \
                 product with, got {body}"
            );
        }

        // None of those refusals touched the page the real product holds.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("{}/{minted}", kind.collection(&owner)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "expected {kind:?} {minted} to survive every refused delete, got {status} {fetched}"
        );
    }
}
