// SPDX-License-Identifier: CC0-1.0

//! `ListProcurementPortalSuppliers`, driven end to end over this service's wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body. Nothing
//! reaches into the state struct, so what these tests see is exactly what an SDK client
//! sees.
//!
//! The rule under test: the suppliers reported for a portal are exactly the supplier
//! identities the stored procurement portal preferences configure AGAINST THAT PORTAL —
//! one per environment a preference describes (its own members are the production
//! environment, its `TestEnvPreference` the test one) and per seller-of-record id its
//! `Selector` selects, each identity reported once however many preferences carry it —
//! walked in one stable order, a page at a time, and reported as nothing at all for a
//! portal no preference configures. A `PortalIdentifier` that names no portal this
//! service serves is the modelled `ResourceNotFoundException`.
//!
//! The tables below span that class rather than one request: all four portals, a
//! preference with no selector and no test environment, one with several
//! seller-of-record ids, one with a test environment naming its own supplier, one with
//! both, two preferences configuring the same supplier identity, every page size
//! including both published `MaxResults` bounds and one past each, several identifiers
//! that name no portal, and the listing re-read after a delete and after a rewrite.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::PROCUREMENT_PORTAL_NAME_VALUES;

/// The portals this service serves, each with the `PortalIdentifier` a caller names it
/// by: one per value of `ProcurementPortalName` (`smithy_query(query="shape",
/// shape="ProcurementPortalName")`, read here from the model's own value set rather than
/// restated), identified by that name lowercased with its underscores hyphenated. The
/// model publishes no identifier for a portal, so this is the identifier the emulator
/// derives and the one `ListProcurementPortals` reports.
fn portals() -> Vec<(&'static str, String)> {
    PROCUREMENT_PORTAL_NAME_VALUES
        .iter()
        .map(|name| (*name, name.to_lowercase().replace('_', "-")))
        .collect()
}

/// The identifier a caller names `portal_name` by, by that same rule.
fn identifier_of(portal_name: &str) -> String {
    portal_name.to_lowercase().replace('_', "-")
}

/// The values `ProcurementPortalEnv` admits (`smithy_query(query="shape",
/// shape="ProcurementPortalEnv")`), which is what a reported `Environment` is one of.
const ENVIRONMENTS: &[&str] = &["PROD", "TEST"];

/// `MaxResults`' published bounds (`smithy_query(query="operation",
/// operation="ListProcurementPortalSuppliers")`: `@range(min: 1, max: 100)`).
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

/// The refusal's text, under either key this service's planes write it with.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The one contact a create carries (`Contacts` is modelled `length 1..1`), with an
/// `Email` of the modelled `EmailString` form.
fn contacts(local_part: &str) -> Value {
    json!([{"Name": "Portal Administrator", "Email": format!("{local_part}@example.com")}])
}

/// `CreateProcurementPortalPreference`, asserting it succeeded, and the ARN it minted.
async fn create(router: &MultiServiceRouter, request: &Value) -> String {
    let created = ok(router, "CreateProcurementPortalPreference", request).await;
    created
        .get("ProcurementPortalPreferenceArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreferenceArn in {created}"))
        .to_string()
}

/// One supplier as this suite compares it: its identifier, its environment and the
/// seller of record it applies to (empty when it applies to all of them).
type Supplier = (String, String, String);

/// `value` as a `Supplier`, asserting the shape of what was reported: a required
/// `SupplierIdentifier`, an `Environment` of the modelled enum, and no `CountryCode` —
/// no member of a stored preference carries a country, so the reader reports none.
fn supplier(case: &str, value: &Value) -> Supplier {
    let identifier = value
        .get("SupplierIdentifier")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a SupplierIdentifier in {value}"))
        .to_string();
    let environment = value
        .get("Environment")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected an Environment in {value}"))
        .to_string();
    assert!(
        ENVIRONMENTS.contains(&environment.as_str()),
        "case {case}: expected an Environment among {ENVIRONMENTS:?}, got {environment:?}"
    );
    assert_eq!(
        value.get("CountryCode"),
        None,
        "case {case}: expected no CountryCode, got {value}"
    );
    (
        identifier,
        environment,
        value
            .get("SellerOfRecord")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// Every supplier `portal_identifier` reports, walked through to the last page with
/// `max_results` per call: the suppliers in the order they were reported, and how many
/// pages the walk took.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    portal_identifier: &str,
    max_results: Option<i64>,
) -> (Vec<Supplier>, usize) {
    let mut reported = Vec::new();
    let mut pages = 0;
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({"PortalIdentifier": portal_identifier});
        if let Some(max_results) = max_results {
            request["MaxResults"] = json!(max_results);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let page = ok(router, "ListProcurementPortalSuppliers", &request).await;
        pages += 1;
        let items = page
            .get("ProcurementPortalSuppliers")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected ProcurementPortalSuppliers in {page}"))
            .clone();
        if let Some(max_results) = max_results {
            assert!(
                i64::try_from(items.len()).unwrap_or(i64::MAX) <= max_results,
                "case {case}: expected at most {max_results} suppliers in a page, got {}",
                items.len()
            );
        }
        reported.extend(items.iter().map(|item| supplier(case, item)));
        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            break;
        }
        assert!(
            pages <= 64,
            "case {case}: the walk did not terminate within 64 pages"
        );
    }
    (reported, pages)
}

/// One `CreateProcurementPortalPreference` request, as a table row, with the suppliers it
/// configures by the rule under test.
#[derive(Debug, Clone)]
struct SupplierCase {
    case: &'static str,
    request: Value,
}

/// The suppliers one create request configures: its own `SupplierIdentifier` in the
/// production environment, its `TestEnvPreference`'s in the test one, each once per
/// seller-of-record id the `Selector` selects and once with no seller of record when it
/// selects none.
fn configured_by(request: &Value) -> Vec<Supplier> {
    let sellers: Vec<String> = match request
        .get("Selector")
        .and_then(|selector| selector.get("SellerOfRecords"))
        .and_then(Value::as_array)
    {
        Some(ids) if !ids.is_empty() => ids
            .iter()
            .map(|id| id.as_str().unwrap_or_default().to_string())
            .collect(),
        _ => vec![String::new()],
    };
    let mut environments = vec![(
        "PROD",
        request
            .get("SupplierIdentifier")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )];
    if let Some(test) = request.get("TestEnvPreference") {
        environments.push((
            "TEST",
            test.get("SupplierIdentifier")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ));
    }
    let mut configured = Vec::new();
    for (environment, identifier) in environments {
        for seller in &sellers {
            configured.push((identifier.clone(), environment.to_string(), seller.clone()));
        }
    }
    configured
}

/// The preferences the rule is asserted over: every portal, a preference with neither a
/// selector nor a test environment, one selecting several seller-of-record ids, one with
/// a test environment naming its own supplier, one with both, two preferences on one
/// portal configuring the SAME supplier identity (so it is reported once) and a third
/// configuring another.
fn supplier_cases() -> Vec<SupplierCase> {
    vec![
        SupplierCase {
            case: "sap, no selector and no test environment",
            request: json!({
                "ProcurementPortalName": "SAP_BUSINESS_NETWORK",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "sap-buyer-1",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "sap-supplier-1",
                "EinvoiceDeliveryEnabled": true,
                "PurchaseOrderRetrievalEnabled": false,
                "Contacts": contacts("sap-1"),
            }),
        },
        SupplierCase {
            case: "sap, another buyer configuring the SAME supplier identity",
            request: json!({
                "ProcurementPortalName": "SAP_BUSINESS_NETWORK",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "sap-buyer-2",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "sap-supplier-1",
                "EinvoiceDeliveryEnabled": true,
                "PurchaseOrderRetrievalEnabled": true,
                "Contacts": contacts("sap-2"),
            }),
        },
        SupplierCase {
            case: "sap, a third preference configuring another supplier",
            request: json!({
                "ProcurementPortalName": "SAP_BUSINESS_NETWORK",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "sap-buyer-3",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "sap-supplier-2",
                "EinvoiceDeliveryEnabled": false,
                "PurchaseOrderRetrievalEnabled": true,
                "Contacts": contacts("sap-3"),
            }),
        },
        SupplierCase {
            case: "sap, a selector of three seller-of-record ids",
            request: json!({
                "ProcurementPortalName": "SAP_BUSINESS_NETWORK",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "AN01000000001",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "AN01000000002",
                "Selector": {"SellerOfRecords": ["AWS-INC", "AMCS-LLC", "AWS-EMEA-SARL"]},
                "EinvoiceDeliveryEnabled": true,
                "PurchaseOrderRetrievalEnabled": true,
                "Contacts": contacts("sap-4"),
            }),
        },
        SupplierCase {
            case: "coupa, a test environment naming its own supplier",
            request: json!({
                "ProcurementPortalName": "COUPA",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "coupa-alt-buyer-1",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "coupa-alt-supplier-prod",
                "TestEnvPreference": {
                    "BuyerDomain": "NetworkID",
                    "BuyerIdentifier": "coupa-alt-buyer-test",
                    "SupplierDomain": "NetworkID",
                    "SupplierIdentifier": "coupa-alt-supplier-test",
                },
                "EinvoiceDeliveryEnabled": true,
                "PurchaseOrderRetrievalEnabled": false,
                "Contacts": contacts("coupa-alt-1"),
            }),
        },
        SupplierCase {
            case: "coupa, a selector and a test environment together",
            request: json!({
                "ProcurementPortalName": "COUPA",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "coupa-buyer-1",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "coupa-supplier-prod",
                "Selector": {"SellerOfRecords": ["AWS-INC", "AMCS-LLC"]},
                "TestEnvPreference": {
                    "BuyerDomain": "NetworkID",
                    "BuyerIdentifier": "coupa-buyer-test",
                    "SupplierDomain": "NetworkID",
                    "SupplierIdentifier": "coupa-supplier-test",
                },
                "EinvoiceDeliveryEnabled": true,
                "PurchaseOrderRetrievalEnabled": true,
                "Contacts": contacts("coupa-1"),
            }),
        },
    ]
}

/// The suppliers every portal owes, by the rule under test, for the creates in `cases`:
/// one entry per supplier identity configured against that portal, deduplicated, in the
/// order a reader walks them (by identifier, then environment, then seller of record).
fn expected_by_portal(cases: &[SupplierCase], portal_name: &str) -> Vec<Supplier> {
    cases
        .iter()
        .filter(|case| case.request.get("ProcurementPortalName") == Some(&json!(portal_name)))
        .flat_map(|case| configured_by(&case.request))
        .collect::<BTreeSet<Supplier>>()
        .into_iter()
        .collect()
}

/// Every preference in `cases`, created in order; each create is asserted to have minted
/// an ARN, so a row that cannot be stored fails on the create rather than on the listing.
async fn create_all(router: &MultiServiceRouter, cases: &[SupplierCase]) {
    for case in cases {
        let arn = create(router, &case.request).await;
        assert!(
            arn.starts_with("arn:aws:invoicing::"),
            "case {}: expected a minted preference ARN, got {arn:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn reports_the_suppliers_the_stored_preferences_configure() {
    let router = emulator();
    let cases = supplier_cases();

    // Nothing is configured yet, so every portal owes an empty listing and no token —
    // not a 404, which is for an identifier that names no portal at all.
    for (portal_name, portal_identifier) in portals() {
        let (reported, pages) = walk(&router, portal_name, &portal_identifier, None).await;
        assert!(
            reported.is_empty() && pages == 1,
            "case {portal_name} before any create: expected one empty page, got {pages} pages \
             of {reported:?}"
        );
    }

    create_all(&router, &cases).await;

    for (portal_name, portal_identifier) in portals() {
        let expected = expected_by_portal(&cases, portal_name);
        let (reported, _) = walk(&router, portal_name, &portal_identifier, None).await;
        assert_eq!(
            reported, expected,
            "case {portal_name}: expected the configured suppliers {expected:?}, got {reported:?}"
        );
        // A read changes nothing a later caller sees: the same walk answers the same.
        let (again, _) = walk(&router, portal_name, &portal_identifier, None).await;
        assert_eq!(
            again, reported,
            "case {portal_name} re-read: expected {reported:?}, got {again:?}"
        );
    }
}

#[tokio::test]
async fn pages_concatenate_to_the_whole_listing() {
    let router = emulator();
    let cases = supplier_cases();
    create_all(&router, &cases).await;

    // Every page size: both published `MaxResults` bounds and the sizes between them that
    // split the listings (six suppliers for one portal of the catalog, five for the other)
    // — one that is a divisor of a listing, one that is not, and one larger than all of
    // them.
    for max_results in [MAX_RESULTS_MIN, 2, 3, 4, 5, MAX_RESULTS_MAX] {
        for (portal_name, portal_identifier) in portals() {
            let case = format!("{portal_name} at MaxResults {max_results}");
            let expected = expected_by_portal(&cases, portal_name);
            let (reported, pages) =
                walk(&router, &case, &portal_identifier, Some(max_results)).await;
            assert_eq!(
                reported, expected,
                "case {case}: expected the pages to concatenate to {expected:?}, got {reported:?}"
            );
            let page_size = usize::try_from(max_results).unwrap_or(usize::MAX);
            let owed_pages = expected.len().div_ceil(page_size).max(1);
            assert_eq!(
                pages, owed_pages,
                "case {case}: expected {owed_pages} pages for {} suppliers, got {pages}",
                expected.len()
            );
        }
    }
}

#[tokio::test]
async fn reports_a_selector_at_the_modelled_seller_of_record_length() {
    let router = emulator();
    // `ProcurementPortalPreferenceSelector$SellerOfRecords` is modelled
    // `@length(min: 0, max: 100)` (`smithy_query(query="shape",
    // shape="ProcurementPortalPreferenceSelector")`), which is what bounds how many
    // suppliers one preference can configure: the published maximum, and one past it.
    const MAX_SELLERS_OF_RECORD: usize = 100;
    let sellers = |count: usize| -> Vec<String> {
        (0..count).map(|index| format!("AWS-SOR-{index:03}")).collect()
    };

    let at_the_maximum = json!({
        "ProcurementPortalName": "SAP_BUSINESS_NETWORK",
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": "sap-buyer-max",
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": "sap-supplier-max",
        "Selector": {"SellerOfRecords": sellers(MAX_SELLERS_OF_RECORD)},
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
        "Contacts": contacts("sap-max"),
    });
    create(&router, &at_the_maximum).await;
    let expected = configured_by(&at_the_maximum)
        .into_iter()
        .collect::<BTreeSet<Supplier>>()
        .into_iter()
        .collect::<Vec<Supplier>>();
    let (reported, pages) = walk(
        &router,
        "at the maximum",
        &identifier_of("SAP_BUSINESS_NETWORK"),
        Some(MAX_RESULTS_MAX),
    )
    .await;
    assert_eq!(
        reported.len(),
        MAX_SELLERS_OF_RECORD,
        "case at the maximum: expected one supplier per selected seller of record, got {}",
        reported.len()
    );
    assert_eq!(
        (reported, pages),
        (expected, 1),
        "case at the maximum: expected the selected sellers of record in one page"
    );

    // One past the published maximum is refused by the modelled length, so no such
    // preference is ever stored for this reader to report.
    let mut past_the_maximum = at_the_maximum.clone();
    past_the_maximum["BuyerIdentifier"] = json!("sap-buyer-past-max");
    past_the_maximum["Selector"] = json!({"SellerOfRecords": sellers(MAX_SELLERS_OF_RECORD + 1)});
    let (status, body) = call(
        &router,
        "CreateProcurementPortalPreference",
        &past_the_maximum,
    )
    .await;
    assert_eq!(
        status, 400,
        "case past the maximum: expected HTTP 400, got {status} {body}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "case past the maximum: expected a ValidationException, got {body}"
    );
}

#[tokio::test]
async fn refuses_a_max_results_outside_the_published_range() {
    let router = emulator();
    for max_results in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let (status, body) = call(
            &router,
            "ListProcurementPortalSuppliers",
            &json!({
                "PortalIdentifier": identifier_of("SAP_BUSINESS_NETWORK"),
                "MaxResults": max_results,
            }),
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
async fn refuses_an_identifier_that_names_no_portal() {
    let router = emulator();
    // Identifiers that name no portal: the portal NAME rather than its identifier, a
    // near-miss of one, an identifier of another shape entirely, and one that is a
    // prefix of a real one.
    for portal_identifier in [
        "COUPA",
        "coupaa",
        "sap_business_network",
        "unknown-portal",
        "cou",
    ] {
        let (status, body) = call(
            &router,
            "ListProcurementPortalSuppliers",
            &json!({"PortalIdentifier": portal_identifier}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {portal_identifier}: expected HTTP 404, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {portal_identifier}: expected a ResourceNotFoundException, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(portal_identifier),
            "case {portal_identifier}: expected the refusal to name the identifier, got {message:?}"
        );
    }
}

#[tokio::test]
async fn refuses_a_next_token_minted_for_another_walk() {
    let router = emulator();
    create_all(&router, &supplier_cases()).await;

    // A token minted for Coupa's walk resumes at a position in COUPA's order, so it is
    // not a token for another portal's walk — nor is anything this service never minted.
    let first = ok(
        &router,
        "ListProcurementPortalSuppliers",
        &json!({"PortalIdentifier": "coupa", "MaxResults": 1}),
    )
    .await;
    let coupa_token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a NextToken in {first}"))
        .to_string();

    let other_portal = identifier_of("SAP_BUSINESS_NETWORK");
    for (case, portal_identifier, next_token) in [
        (
            "coupa's token on another portal's walk",
            other_portal.as_str(),
            coupa_token.as_str(),
        ),
        ("a token this service never minted", "coupa", "not-a-token"),
        ("hex that decodes to another prefix", "coupa", "6162636465"),
    ] {
        let (status, body) = call(
            &router,
            "ListProcurementPortalSuppliers",
            &json!({"PortalIdentifier": portal_identifier, "NextToken": next_token}),
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
        // The token stays good for the walk it WAS minted for.
        let resumed = ok(
            &router,
            "ListProcurementPortalSuppliers",
            &json!({"PortalIdentifier": "coupa", "NextToken": coupa_token}),
        )
        .await;
        assert!(
            resumed
                .get("ProcurementPortalSuppliers")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty()),
            "case {case}: expected coupa's own token to resume its walk, got {resumed}"
        );
    }
}

#[tokio::test]
async fn follows_the_preferences_through_a_rewrite_and_a_delete() {
    let router = emulator();
    let create_request = json!({
        "ProcurementPortalName": "SAP_BUSINESS_NETWORK",
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": "sap-buyer-1",
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": "sap-supplier-1",
        "Selector": {"SellerOfRecords": ["AWS-INC"]},
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
        "Contacts": contacts("sap-rewrite"),
    });
    let arn = create(&router, &create_request).await;
    let portal_identifier = identifier_of("SAP_BUSINESS_NETWORK");
    let (created, _) = walk(&router, "after the create", &portal_identifier, None).await;
    assert_eq!(
        created,
        configured_by(&create_request),
        "case after the create: expected the created preference's supplier, got {created:?}"
    );

    // A rewrite of the preference is what changes the suppliers reported: the listing is
    // computed from state, so it follows the configuration rather than the first answer.
    // `PutProcurementPortalPreference`'s input carries neither the portal nor the
    // buyer/supplier pair (`smithy_query(query="operation",
    // operation="PutProcurementPortalPreference")`), so the production supplier identity
    // is the one the create stored and the rewrite changes the selector and the test
    // environment around it.
    let rewrite = json!({
        "ProcurementPortalPreferenceArn": arn,
        "Selector": {"SellerOfRecords": ["AWS-INC", "AMCS-LLC"]},
        "TestEnvPreference": {
            "BuyerDomain": "NetworkID",
            "BuyerIdentifier": "sap-buyer-test",
            "SupplierDomain": "NetworkID",
            "SupplierIdentifier": "sap-supplier-test",
        },
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
        "Contacts": contacts("sap-rewritten"),
    });
    ok(&router, "PutProcurementPortalPreference", &rewrite).await;
    let (rewritten, _) = walk(&router, "after the rewrite", &portal_identifier, None).await;
    // What the preference configures after the rewrite: the stored production supplier
    // identity, under the rewritten selector and test environment.
    let mut rewritten_configuration = rewrite.clone();
    rewritten_configuration["SupplierIdentifier"] =
        create_request["SupplierIdentifier"].clone();
    let expected = configured_by(&rewritten_configuration)
        .into_iter()
        .collect::<BTreeSet<Supplier>>()
        .into_iter()
        .collect::<Vec<Supplier>>();
    assert_eq!(
        rewritten, expected,
        "case after the rewrite: expected {expected:?}, got {rewritten:?}"
    );

    // And a delete takes its suppliers with it: nothing configures that portal any more.
    ok(
        &router,
        "DeleteProcurementPortalPreference",
        &json!({"ProcurementPortalPreferenceArn": arn}),
    )
    .await;
    let (after_delete, pages) = walk(&router, "after the delete", &portal_identifier, None).await;
    assert!(
        after_delete.is_empty() && pages == 1,
        "case after the delete: expected one empty page, got {pages} pages of {after_delete:?}"
    );
}
