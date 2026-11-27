// SPDX-License-Identifier: CC0-1.0

//! The one page walk every paginated listing of this service answers, driven end to end
//! over the wire through EVERY listing that answers it.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body. The one
//! thing this suite reaches into the state struct for is SEEDING the invoice registry — no
//! operation of this service's API issues an invoice, so there is no request that could put
//! one there — and everything the listings answer is then read back over the wire.
//!
//! The rule: a page holds at most the page size the request's `MaxResults` resolved to
//! inside that listing's own maximum, taken in the listing's order from the position the
//! caller's `NextToken` named, and it carries a token naming the first entry that did NOT
//! fit — so a page before the last holds exactly the page size, the last page carries no
//! token, and the pages of a walk concatenate to the unpaged answer exactly once, whatever
//! the page size.
//!
//! The table drives that rule over every listing — the two keyed by a minted ARN
//! (`ListInvoiceUnits`, `ListProcurementPortalPreferences`), the one keyed by an invoice id
//! (`ListInvoiceSummaries`), the one over the portal catalog (`ListProcurementPortals`) and
//! the one whose token names an ORDINAL (`ListProcurementPortalSuppliers`) — at page sizes
//! below, at and above each listing's entry count, at its published `MaxResults` maximum,
//! and with `MaxResults` omitted.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::InvoiceSummary;

/// `MaxResults`' published maximum for the invoice-unit listing (`MaxResultsInteger`
/// carries `@range(min: 1, max: 500)`: `smithy_query(query="operation",
/// operation="ListInvoiceUnits")`).
const INVOICE_UNITS_MAX_RESULTS: i64 = 500;

/// `MaxResults`' published maximum for every other listing of this service
/// (`@range(min: 1, max: 100)`: `smithy_query(query="operation",
/// operation="ListInvoiceSummaries,ListProcurementPortals,ListProcurementPortalPreferences,ListProcurementPortalSuppliers")`).
const HUNDRED_MAX_RESULTS: i64 = 100;

/// The account the seeded invoices are issued to, which the `ListInvoiceSummaries` row
/// selects.
const INVOICE_ACCOUNT: &str = "111111111111";

/// The portal the `ListProcurementPortalSuppliers` row walks, by the `PortalIdentifier` a
/// caller names it with (the `ProcurementPortalName` value lowercased with its underscores
/// hyphenated, as `ListProcurementPortals` reports it).
const SUPPLIERS_PORTAL_IDENTIFIER: &str = "coupa";

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services. The engine comes back so a test
/// can seed the invoice registry the summaries listing reads.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
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

/// The one contact a preference write carries (`Contacts` is modelled
/// `@length(min: 1, max: 1)`).
fn contacts(identifier: &str) -> Value {
    json!([{"Name": "Portal Administrator", "Email": format!("{identifier}@example.com")}])
}

/// The invoice units this suite lists, created over the wire: rows differing along the
/// members the listing reports, so the pages carry entries that are not interchangeable.
async fn create_invoice_units(router: &MultiServiceRouter) {
    let units: &[(&str, &str, &[&str])] = &[
        ("alpha-unit", "123456789012", &[]),
        ("beta unit 2", "223456789012", &["333333333333"]),
        ("Gämma_Einheit-3", "323456789012", &["444444444444", "555555555555"]),
        ("delta", "423456789012", &[]),
        ("epsilon/5", "523456789012", &["666666666666"]),
    ];
    for (index, (name, receiver, linked)) in units.iter().enumerate() {
        ok(
            router,
            "CreateInvoiceUnit",
            &json!({
                "Name": name,
                "InvoiceReceiver": receiver,
                "Rule": {"LinkedAccounts": linked},
                "ClientToken": format!("unit-{index}"),
            }),
        )
        .await;
    }
}

/// The procurement portal preferences this suite lists, created over the wire. They are
/// also what the supplier listing is derived from, so one of them selects several
/// seller-of-record ids and names a test environment — which is what gives that walk more
/// entries than the preferences themselves.
async fn create_procurement_portal_preferences(router: &MultiServiceRouter) {
    // (the portal, the identity suffix, the selected seller-of-record ids, whether the
    // preference names a test environment)
    let preferences: &[(&str, &str, &[&str], bool)] = &[
        ("COUPA", "coupa-1", &["AWS-INC", "AMCS-LLC", "AWS-EMEA"], true),
        ("COUPA", "coupa-2", &[], false),
        ("SAP_BUSINESS_NETWORK", "sap-1", &["AWS-INC", "AWS-EMEA"], false),
        ("SAP_BUSINESS_NETWORK", "sap-2", &["AWS-INC"], false),
        ("SAP_BUSINESS_NETWORK", "sap-3", &[], true),
    ];
    for (portal, identifier, sellers, test_env) in preferences {
        let mut request = json!({
            "ProcurementPortalName": portal,
            "BuyerDomain": "NetworkID",
            "BuyerIdentifier": format!("{identifier}-buyer"),
            "SupplierDomain": "NetworkID",
            "SupplierIdentifier": format!("{identifier}-supplier"),
            "EinvoiceDeliveryEnabled": true,
            "PurchaseOrderRetrievalEnabled": true,
            "Contacts": contacts(identifier),
        });
        if !sellers.is_empty() {
            request["Selector"] = json!({"SellerOfRecords": sellers});
        }
        if *test_env {
            request["TestEnvPreference"] = json!({
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": format!("{identifier}-buyer-test"),
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": format!("{identifier}-supplier-test"),
            });
        }
        ok(router, "CreateProcurementPortalPreference", &request).await;
    }
}

/// Put invoices in the service's registry, keyed by invoice id: an invoice is issued by
/// AWS out of a billing period's activity and no operation of this service's API creates
/// one, so seeding is the only way to give that listing something to page over.
fn seed_invoices(engine: &engine::InMemoryEngine) {
    let state = service_com_amazonaws_invoicing_invoicing::state::service_state(engine)
        .expect("the shared engine holds this service's state");
    let mut store = state.lock();
    for index in 1..=5 {
        let invoice_id = format!("INV-{index:03}");
        store.invoices.insert(
            invoice_id.clone(),
            service_com_amazonaws_invoicing_invoicing::state::StoredInvoice::new(InvoiceSummary {
                account_id: Some(INVOICE_ACCOUNT.to_string()),
                invoice_id: Some(invoice_id),
                ..Default::default()
            }),
        );
    }
}

/// One paginated listing, as a table row: the operation to drive, the output member its
/// entries arrive under, the published maximum its page size is bounded by, and everything
/// its request carries besides `MaxResults` and `NextToken`.
#[derive(Debug)]
struct Listing {
    operation: &'static str,
    items_member: &'static str,
    max_page_size: i64,
    request: Value,
}

/// Every paginated listing this service serves.
fn listings() -> Vec<Listing> {
    vec![
        Listing {
            operation: "ListInvoiceUnits",
            items_member: "InvoiceUnits",
            max_page_size: INVOICE_UNITS_MAX_RESULTS,
            request: json!({}),
        },
        Listing {
            operation: "ListProcurementPortalPreferences",
            items_member: "ProcurementPortalPreferences",
            max_page_size: HUNDRED_MAX_RESULTS,
            request: json!({}),
        },
        Listing {
            operation: "ListProcurementPortals",
            items_member: "ProcurementPortals",
            max_page_size: HUNDRED_MAX_RESULTS,
            request: json!({}),
        },
        Listing {
            operation: "ListProcurementPortalSuppliers",
            items_member: "ProcurementPortalSuppliers",
            max_page_size: HUNDRED_MAX_RESULTS,
            request: json!({"PortalIdentifier": SUPPLIERS_PORTAL_IDENTIFIER}),
        },
        Listing {
            operation: "ListInvoiceSummaries",
            items_member: "InvoiceSummaries",
            max_page_size: HUNDRED_MAX_RESULTS,
            request: json!({
                "Selector": {"ResourceType": "ACCOUNT_ID", "Value": INVOICE_ACCOUNT},
            }),
        },
    ]
}

/// What one walk of a listing reported: the entries of its pages in order, and how many
/// pages it took.
#[derive(Debug)]
struct Walk {
    items: Vec<Value>,
    pages: usize,
}

impl Listing {
    /// The entries of one page of this listing's response.
    fn items_of(&self, page: &Value) -> Vec<Value> {
        page.get(self.items_member)
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!(
                    "case {}: expected a {} array in {page}",
                    self.operation, self.items_member
                )
            })
            .clone()
    }

    /// Walk this listing to its end at `page_size` (`None` for a request that omits
    /// `MaxResults`), asserting the cut and the token as the walk goes: a page holds at
    /// most the page size, a page that carries a token holds exactly it, and the walk
    /// terminates.
    async fn walk(&self, router: &MultiServiceRouter, page_size: Option<i64>, limit: usize) -> Walk {
        let case = format!("{} at MaxResults {page_size:?}", self.operation);
        let mut items: Vec<Value> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = self.request.clone();
            if let Some(size) = page_size {
                request["MaxResults"] = json!(size);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let page = ok(router, self.operation, &request).await;
            let reported = self.items_of(&page);
            let size = usize::try_from(page_size.unwrap_or(self.max_page_size))
                .expect("a published page size is a positive count");
            assert!(
                reported.len() <= size,
                "case {case}: a page holds at most {size} entries, got {}",
                reported.len()
            );
            next_token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            // The page cuts AT the page size: a page that leaves something to resume at
            // is a full one, so a short page is always the last of the walk.
            if next_token.is_some() {
                assert_eq!(
                    reported.len(),
                    size,
                    "case {case}: a page carrying a NextToken holds exactly {size} entries, got {}",
                    reported.len()
                );
            }
            items.extend(reported);
            pages += 1;
            assert!(
                pages <= limit + 1,
                "case {case}: the walk must terminate, got {pages} pages of {items:?}"
            );
            if next_token.is_none() {
                break;
            }
        }
        Walk { items, pages }
    }
}

/// Every listing answers one page walk: at every page size the pages concatenate to the
/// unpaged answer, in order and exactly once, cut into the owed number of pages.
#[tokio::test]
async fn every_listing_pages_by_the_one_shared_rule() {
    let (router, engine) = emulator();
    create_invoice_units(&router).await;
    create_procurement_portal_preferences(&router).await;
    seed_invoices(&engine);

    for listing in listings() {
        let operation = listing.operation;

        // The whole answer, from a request that omits `MaxResults` and so asks for a full
        // page: every listing here holds fewer entries than its published maximum, so this
        // is one page with nothing left to resume at.
        let whole = listing.walk(&router, None, HUNDRED_MAX_RESULTS as usize).await;
        assert_eq!(
            whole.pages, 1,
            "case {operation} with MaxResults omitted: expected the whole answer in one page, got {} pages of {:?}",
            whole.pages, whole.items
        );
        assert!(
            whole.items.len() > 1,
            "case {operation}: this suite must leave something to page over, got {:?}",
            whole.items
        );
        let distinct: BTreeSet<String> = whole
            .items
            .iter()
            .map(|item| item.to_string())
            .collect();
        assert_eq!(
            distinct.len(),
            whole.items.len(),
            "case {operation}: a listing reports each entry once, got {:?}",
            whole.items
        );

        // Page sizes below, at and above the entry count, plus the published maximum.
        let mut page_sizes: Vec<i64> = vec![1, 2, 3, 4, 5, listing.max_page_size];
        page_sizes.dedup();
        for page_size in page_sizes {
            let walked = listing.walk(&router, Some(page_size), whole.items.len()).await;
            assert_eq!(
                walked.items, whole.items,
                "case {operation} at MaxResults {page_size}: the pages must concatenate to the unpaged answer in order, got {:?}",
                walked.items
            );
            let size = usize::try_from(page_size).expect("a page size is a positive count");
            let owed_pages = whole.items.len().div_ceil(size).max(1);
            assert_eq!(
                walked.pages, owed_pages,
                "case {operation} at MaxResults {page_size}: expected {owed_pages} pages for {} entries, got {}",
                whole.items.len(),
                walked.pages
            );
        }
    }
}
