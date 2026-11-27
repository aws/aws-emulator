// SPDX-License-Identifier: CC0-1.0

//! `ListProcurementPortals`, driven end to end over this service's wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body. Nothing
//! reaches into the state struct, so what these tests see is exactly what an SDK client
//! sees.
//!
//! The rule under test: the portals reported are exactly the portals the SERVICE
//! supports — one entry per value of the modelled `ProcurementPortalName`, whatever a
//! caller has stored — each carrying the identifier that portal is named by on the rest
//! of this plane, its portal name, and default feature configurations whose document and
//! attachment types are the ones the corresponding modelled enums admit. The listing is
//! walked in one stable order, a page at a time, bounded by `MaxResults`, resumed only by
//! a token this walk minted, and a read leaves what a later caller sees unchanged.
//!
//! The tables below span that class rather than one request: every portal, every page
//! size that splits the listing plus both published `MaxResults` bounds and one past
//! each, several tokens this walk never minted, the listing re-read after a read and
//! after a preference create, and each reported identifier handed back to
//! `ListProcurementPortalSuppliers` beside identifiers no portal answers to.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::PROCUREMENT_PORTAL_NAME_VALUES;

/// The portals this service serves, each with the `PortalIdentifier` a caller names it
/// by: one per value of `ProcurementPortalName` (`smithy_query(query="shape",
/// shape="ProcurementPortalName")`, read here from the model's own value set rather than
/// restated), identified by that name lowercased with its underscores hyphenated. The
/// model publishes no identifier for a portal, so this is the identifier the emulator
/// derives, and it is the one `ListProcurementPortalSuppliers` already resolves a portal
/// by.
///
/// In the order the listing owes them — by identifier — so a row here is also the
/// position a page resumes at.
fn portals() -> Vec<(String, &'static str)> {
    let mut portals: Vec<(String, &'static str)> = PROCUREMENT_PORTAL_NAME_VALUES
        .iter()
        .map(|name| (name.to_lowercase().replace('_', "-"), *name))
        .collect();
    portals.sort_by(|left, right| left.0.cmp(&right.0));
    portals
}

/// The values `EinvoiceDeliveryDocumentType` admits (`smithy_query(query="shape",
/// shape="EinvoiceDeliveryDocumentType")`), which is what a portal's default
/// `DocumentTypes` is drawn from: the model publishes no per-portal subset, so the
/// default is every one of them. Five values, inside `EinvoiceDeliveryDocumentTypes`'
/// `@length(min: 0, max: 10)`.
const DOCUMENT_TYPES: &[&str] = &[
    "AWS_CLOUD_CREDIT_MEMO",
    "AWS_CLOUD_INVOICE",
    "AWS_MARKETPLACE_CREDIT_MEMO",
    "AWS_MARKETPLACE_INVOICE",
    "AWS_REQUEST_FOR_PAYMENT",
];

/// The values `EinvoiceDeliveryAttachmentType` admits (`smithy_query(query="shape",
/// shape="EinvoiceDeliveryAttachmentType")`), which a portal's default `AttachmentTypes`
/// is drawn from, for the same reason.
const ATTACHMENT_TYPES: &[&str] = &["INVOICE_PDF", "RFP_PDF"];

/// `MaxResults`' published bounds (`smithy_query(query="operation",
/// operation="ListProcurementPortals")`: `@range(min: 1, max: 100)`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        engine,
    ));
    router
}

/// One awsJson1_0 request, returning the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("Invoicing.{operation}"))
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same call, asserting it succeeded.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} {parsed}"
    );
    parsed
}

/// The wire error code of a failure body (`__type` on the awsJson protocols).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One reported portal, as a client reads it off the wire: the three members the
/// operation documents an entry includes ("the portal identifier, name, and default
/// feature configurations"). `PortalDisplayName` is display prose the model carries for
/// no portal, so these tests assert nothing about it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Portal {
    identifier: String,
    name: String,
    document_types: Vec<String>,
    attachment_types: Vec<String>,
}

/// One entry of a `ProcurementPortals` array, read strictly: a member the operation
/// documents as part of every entry is a failure of the case when it is missing.
fn portal(case: &str, item: &Value) -> Portal {
    let strings = |member: &str, from: Option<&Value>| -> Vec<String> {
        from.and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected a {member} list in {item}"))
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: expected a {member} string in {item}"))
                    .to_string()
            })
            .collect()
    };
    let invoice_configuration = item
        .get("DefaultFeatureConfigurations")
        .and_then(|configurations| configurations.get("InvoiceConfiguration"))
        .unwrap_or_else(|| {
            panic!("case {case}: expected DefaultFeatureConfigurations.InvoiceConfiguration in {item}")
        });
    Portal {
        identifier: item
            .get("PortalIdentifier")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: expected a PortalIdentifier in {item}"))
            .to_string(),
        name: item
            .get("PortalName")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: expected a PortalName in {item}"))
            .to_string(),
        document_types: strings("DocumentTypes", invoice_configuration.get("DocumentTypes")),
        attachment_types: strings(
            "AttachmentTypes",
            invoice_configuration.get("AttachmentTypes"),
        ),
    }
}

/// The portals the rule owes, in the order the listing owes them.
fn expected_portals() -> Vec<Portal> {
    portals()
        .into_iter()
        .map(|(identifier, name)| Portal {
            identifier,
            name: name.to_string(),
            document_types: DOCUMENT_TYPES.iter().map(|t| (*t).to_string()).collect(),
            attachment_types: ATTACHMENT_TYPES.iter().map(|t| (*t).to_string()).collect(),
        })
        .collect()
}

/// Every portal the service reports, walked through to the last page with `max_results`
/// per call: the portals in the order they were reported, and how many pages the walk
/// took.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    max_results: Option<i64>,
) -> (Vec<Portal>, usize) {
    let mut reported = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut request = json!({});
        if let Some(max_results) = max_results {
            request["MaxResults"] = json!(max_results);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let page = ok(router, "ListProcurementPortals", &request).await;
        pages += 1;
        let items = page
            .get("ProcurementPortals")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected a ProcurementPortals array in {page}"))
            .clone();
        if let Some(max_results) = max_results {
            assert!(
                i64::try_from(items.len()).unwrap_or(i64::MAX) <= max_results,
                "case {case}: expected at most {max_results} portals in a page, got {}",
                items.len()
            );
        }
        reported.extend(items.iter().map(|item| portal(case, item)));
        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            break;
        }
        assert!(
            pages <= portals().len() + 1,
            "case {case}: the walk did not terminate after {pages} pages, reported {reported:?}"
        );
    }
    (reported, pages)
}

/// A create of one procurement portal preference against `portal_name`, so a test can
/// show that what a caller STORES does not change the portals the service reports.
fn preference_request(portal_name: &str) -> Value {
    json!({
        "ProcurementPortalName": portal_name,
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": "portals-buyer-1",
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": "portals-supplier-1",
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": false,
        "EinvoiceDeliveryPreference": {
            "EinvoiceDeliveryDocumentTypes": ["AWS_CLOUD_INVOICE"],
            "Protocol": "CXML",
            "PurchaseOrderDataSources": [{
                "EinvoiceDeliveryDocumentType": "AWS_CLOUD_INVOICE",
                "PurchaseOrderDataSourceType": "PURCHASE_ORDER_NOT_REQUIRED"
            }],
            "ConnectionTestingMethod": "TEST_ENV_REPLAY_TEST",
            "EinvoiceDeliveryActivationDate": 1_760_000_000
        },
        "Contacts": [{"Name": "Portal Administrator", "Email": "portals@example.com"}]
    })
}

#[tokio::test]
async fn reports_every_modelled_portal_once() {
    let router = emulator();
    let expected = expected_portals();

    // The default page (`MaxResults` omitted, so the modelled default of 100) holds the
    // whole catalog, in identifier order, with no token left over.
    let page = ok(&router, "ListProcurementPortals", &json!({})).await;
    assert_eq!(
        page.get("NextToken"),
        None,
        "case default page: expected no NextToken once every portal is reported, got {page}"
    );
    let reported: Vec<Portal> = page
        .get("ProcurementPortals")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a ProcurementPortals array in {page}"))
        .iter()
        .map(|item| portal("default page", item))
        .collect();
    assert_eq!(
        reported, expected,
        "case default page: expected {expected:?}, got {reported:?}"
    );
}

#[tokio::test]
async fn pages_concatenate_to_the_whole_listing() {
    let router = emulator();
    let expected = expected_portals();

    // Every page size: both published `MaxResults` bounds and each size in between that
    // splits the catalog, plus the walk that asks for no size at all.
    for max_results in [
        None,
        Some(MAX_RESULTS_MIN),
        Some(2),
        Some(3),
        Some(4),
        Some(5),
        Some(MAX_RESULTS_MAX),
    ] {
        let case = format!("MaxResults {max_results:?}");
        let (reported, pages) = walk(&router, &case, max_results).await;
        assert_eq!(
            reported, expected,
            "case {case}: expected {expected:?}, got {reported:?}"
        );
        let page_size = max_results.unwrap_or(MAX_RESULTS_MAX).max(1) as usize;
        let expected_pages = expected.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected {expected_pages} pages, got {pages}"
        );
    }
}

#[tokio::test]
async fn refuses_a_max_results_outside_the_published_range() {
    let router = emulator();
    for max_results in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let (status, body) = call(
            &router,
            "ListProcurementPortals",
            &json!({"MaxResults": max_results}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case MaxResults {max_results}: expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case MaxResults {max_results}: expected a ValidationException, got {body}"
        );
    }
}

#[tokio::test]
async fn refuses_a_next_token_this_walk_did_not_mint() {
    let router = emulator();

    // The token this walk DID mint, which stays good for it throughout.
    let first = ok(
        &router,
        "ListProcurementPortals",
        &json!({"MaxResults": MAX_RESULTS_MIN}),
    )
    .await;
    let own_token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a NextToken in {first}"))
        .to_string();

    for (case, next_token) in [
        ("a token this service never minted", "not-a-token"),
        ("hex that decodes to another prefix", "6162636465"),
        ("hex of this service's other walk", &hex_of("invoice-units:0")),
    ] {
        let (status, body) = call(
            &router,
            "ListProcurementPortals",
            &json!({"NextToken": next_token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            body.get("Reason").or_else(|| body.get("reason")),
            Some(&json!("invalidNextToken")),
            "case {case}: expected the modelled invalidNextToken reason, got {body}"
        );
        // The refusal of a foreign token leaves this walk's own token good.
        let resumed = ok(
            &router,
            "ListProcurementPortals",
            &json!({"NextToken": own_token}),
        )
        .await;
        let reported: Vec<Portal> = resumed
            .get("ProcurementPortals")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected a ProcurementPortals array in {resumed}"))
            .iter()
            .map(|item| portal(case, item))
            .collect();
        assert_eq!(
            reported,
            expected_portals()[1..].to_vec(),
            "case {case}: expected this walk's own token to resume after the first portal, got {reported:?}"
        );
    }
}

/// The hex a `NextToken` of this service's walks is encoded as, so a test can hand this
/// walk a token belonging to another one.
fn hex_of(plain: &str) -> String {
    plain.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test]
async fn the_catalog_is_what_the_rest_of_the_plane_resolves_a_portal_by() {
    let router = emulator();
    let before = walk(&router, "before any write", None).await.0;

    // Every identifier reported is an identifier this plane answers for: the supplier
    // listing resolves each one, and refuses the ones no portal is reported under.
    for portal in &before {
        let listing = ok(
            &router,
            "ListProcurementPortalSuppliers",
            &json!({"PortalIdentifier": portal.identifier}),
        )
        .await;
        assert!(
            listing
                .get("ProcurementPortalSuppliers")
                .and_then(Value::as_array)
                .is_some(),
            "case {}: expected a supplier listing for a reported portal, got {listing}",
            portal.identifier
        );
    }
    for unreported in ["COUPA", "coupa-sandbox", "sap_business_network", "zi"] {
        let (status, body) = call(
            &router,
            "ListProcurementPortalSuppliers",
            &json!({"PortalIdentifier": unreported}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {unreported}: expected HTTP 404 for an identifier no portal is reported under, got {status} {body}"
        );
    }

    // A read changes nothing a later caller sees, and neither does what a caller STORES:
    // the catalog is the portals the service supports, not the ones an account configured.
    let again = walk(&router, "re-read", None).await.0;
    assert_eq!(
        again, before,
        "case re-read: expected {before:?}, got {again:?}"
    );
    let created = ok(
        &router,
        "CreateProcurementPortalPreference",
        // One of the portals the model declares: which one it is does not matter, since
        // the rule is that storing a preference changes no reported portal.
        &preference_request(portals()[0].1),
    )
    .await;
    assert!(
        created
            .get("ProcurementPortalPreferenceArn")
            .and_then(Value::as_str)
            .is_some_and(|arn| arn.starts_with("arn:aws:invoicing::")),
        "expected a minted preference ARN, got {created}"
    );
    let after = walk(&router, "after a preference create", None).await.0;
    assert_eq!(
        after, before,
        "case after a preference create: expected {before:?}, got {after:?}"
    );
}
