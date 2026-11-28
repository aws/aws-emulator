// SPDX-License-Identifier: CC0-1.0

//! `purchaseIndicators`: the purchase conditions `GetListing` and `GetProduct` report.
//!
//! The rule under test: an indicator RESTATES something about the purchase options the record
//! is sold through, so it is derived from the offers a buyer can actually accept rather than
//! stored beside them —
//!
//! * `NOT_AVAILABLE_FOR_SUBSCRIPTION` ("the product is not available for new subscriptions")
//!   exactly when no offer of the record is still on sale, either because every one of them
//!   has expired or because the record has none;
//! * `AVAILABLE_FOR_USE_WITHOUT_SUBSCRIPTION` ("the product can be used without a
//!   subscription") exactly when some offer still on sale is priced under a model that
//!   charges nothing for the software itself, which is `FREE` or `BYOL`;
//! * neither — "[t]he list is empty when no special purchase conditions apply" — for a record
//!   sold only under a paid offer that is still on sale.
//!
//! Asserted over EVERY listing and product the catalog's first page reports, so the rows span
//! records sold under each pricing model the catalog offers, records whose only offer has
//! expired, and records sold through several offers at once; the expectation for each row is
//! recomputed from the wire — the offers the record reports, which of them this service says
//! are still available (`AVAILABILITY_STATUS`), and each one's own `pricingModel` — never
//! from a value written into the suite.
//!
//! Every suite drives real HTTP requests through the `MultiServiceRouter`, so the protocol,
//! the routing table and the handlers are all exercised.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::ops::types::PURCHASE_INDICATOR_TYPE_VALUES;

use crate::catalog_drilldown::{array, ok, router, text};

/// The pricing models under which the software itself is not charged for.
///
/// Source: `SearchFilter$filterValues` for `PRICING_MODEL` — "One or more of `BYOL`,
/// `CONTRACT`, `FREE`, and `USAGE`" — of which `FREE` charges nothing and `BYOL` is satisfied
/// by a licence the buyer already holds.
const UNSUBSCRIBED_USE: [&str; 2] = ["BYOL", "FREE"];

/// The indicator for a record a buyer may use without subscribing.
const WITHOUT_SUBSCRIPTION: &str = "AVAILABLE_FOR_USE_WITHOUT_SUBSCRIPTION";

/// The indicator for a record no buyer can subscribe to any more.
const NOT_AVAILABLE: &str = "NOT_AVAILABLE_FOR_SUBSCRIPTION";

/// The page size the reads below ask for: the maximum `MaxResults` takes, so one page carries
/// the catalog's first page of records.
const WHOLE_PAGE: i64 = 100;

/// The purchase option ids this service reports as still on sale, read from the service
/// itself rather than computed from a clock: `AVAILABILITY_STATUS` is the filter the model
/// declares for exactly that question.
async fn available_purchase_options(router: &MultiServiceRouter) -> BTreeSet<String> {
    let mut available: BTreeSet<String> = BTreeSet::new();
    let mut next: Option<String> = None;
    loop {
        let mut input = json!({
            "maxResults": WHOLE_PAGE,
            "filters": [{
                "filterType": "AVAILABILITY_STATUS",
                "filterValues": ["AVAILABLE"],
            }],
        });
        if let Some(token) = &next {
            input["nextToken"] = json!(token);
        }
        let body = ok(router, "listPurchaseOptions", &input).await;
        for option in array(&body, "purchaseOptions") {
            available.insert(text(&option, "purchaseOptionId"));
        }
        next = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next.is_none() {
            break;
        }
    }
    assert!(
        !available.is_empty(),
        "case setup: the catalog must hold purchase options still on sale"
    );
    available
}

/// The indicators the rule expects for a record sold through `offer_ids`, given the offers
/// that are still on sale and each offer's pricing model.
fn expected_indicators(
    offer_ids: &BTreeSet<String>,
    available: &BTreeSet<String>,
    pricing_models: &BTreeMap<String, String>,
) -> Vec<String> {
    let on_sale: Vec<&String> = offer_ids
        .iter()
        .filter(|offer_id| available.contains(*offer_id))
        .collect();
    if on_sale.is_empty() {
        return vec![NOT_AVAILABLE.to_string()];
    }
    if on_sale.iter().any(|offer_id| {
        pricing_models
            .get(*offer_id)
            .is_some_and(|model| UNSUBSCRIBED_USE.contains(&model.as_str()))
    }) {
        return vec![WITHOUT_SUBSCRIPTION.to_string()];
    }
    Vec::new()
}

/// The indicator types a response reports, in order.
fn reported(body: &Value) -> Vec<String> {
    array(body, "purchaseIndicators")
        .iter()
        .map(|indicator| text(indicator, "indicatorType"))
        .collect()
}

/// RULE: every listing's and every product's `purchaseIndicators` is what the offers the
/// record is sold through imply, and every indicator it reports is a modelled
/// `PurchaseIndicatorType` value.
#[tokio::test]
async fn the_indicators_a_record_reports_follow_the_offers_it_is_sold_through() {
    let router = router();
    let available = available_purchase_options(&router).await;
    let page = ok(
        &router,
        "searchListings",
        &json!({ "maxResults": WHOLE_PAGE }),
    )
    .await;
    let summaries = array(&page, "listingSummaries");
    assert!(
        !summaries.is_empty(),
        "case setup: the catalog must report listings to read indicators from"
    );

    // Every offer's pricing model, read once per offer.
    let mut pricing_models: BTreeMap<String, String> = BTreeMap::new();
    // How many rows each outcome of the rule was exercised by, so the suite cannot pass by
    // only ever seeing one of them.
    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();

    for summary in &summaries {
        let listing_id = text(summary, "listingId");
        let listing = ok(
            &router,
            "getListing",
            &json!({ "listingId": listing_id.clone() }),
        )
        .await;

        // The offers the listing associates, and the products they cover.
        let mut listing_offers: BTreeSet<String> = BTreeSet::new();
        let mut products: BTreeSet<String> = BTreeSet::new();
        let mut offers_of_product: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for entity in array(&listing, "associatedEntities") {
            let offer_id = entity
                .get("offer")
                .map(|offer| text(offer, "offerId"))
                .unwrap_or_default();
            let product_id = entity
                .get("product")
                .map(|product| text(product, "productId"))
                .unwrap_or_default();
            if !product_id.is_empty() {
                products.insert(product_id.clone());
            }
            if offer_id.is_empty() {
                continue;
            }
            listing_offers.insert(offer_id.clone());
            if !pricing_models.contains_key(&offer_id) {
                let offer = ok(
                    &router,
                    "getOffer",
                    &json!({ "offerId": offer_id.clone() }),
                )
                .await;
                let model = offer
                    .get("pricingModel")
                    .map(|model| text(model, "pricingModelType"))
                    .unwrap_or_default();
                assert!(
                    !model.is_empty(),
                    "case {offer_id}: an offer reports the pricing model it is sold under, got \
                     {offer}"
                );
                pricing_models.insert(offer_id.clone(), model);
            }
            if !product_id.is_empty() {
                offers_of_product
                    .entry(product_id)
                    .or_default()
                    .insert(offer_id);
            }
        }

        let expected = expected_indicators(&listing_offers, &available, &pricing_models);
        let actual = reported(&listing);
        assert_eq!(
            actual, expected,
            "case {listing_id}: expected {expected:?} from the offers {listing_offers:?}, got \
             {actual:?}"
        );
        *outcomes
            .entry(expected.first().cloned().unwrap_or_else(|| "none".to_string()))
            .or_insert(0) += 1;
        for indicator in &actual {
            assert!(
                PURCHASE_INDICATOR_TYPE_VALUES.contains(&indicator.as_str()),
                "case {listing_id}: {indicator:?} is outside PurchaseIndicatorType"
            );
        }

        // The same rule over the listing's PRODUCTS, each judged against the offers that
        // cover it rather than against the listing's whole offer set.
        for product_id in products {
            let product = ok(
                &router,
                "getProduct",
                &json!({ "productId": product_id.clone() }),
            )
            .await;
            let covering = offers_of_product
                .get(&product_id)
                .cloned()
                .unwrap_or_default();
            let expected = expected_indicators(&covering, &available, &pricing_models);
            let actual = reported(&product);
            assert_eq!(
                actual, expected,
                "case {product_id}: expected {expected:?} from the offers {covering:?}, got \
                 {actual:?}"
            );
        }
    }

    assert!(
        outcomes.len() > 2,
        "case setup: the catalog must exercise every outcome of the rule — an unavailable \
         record, one usable without a subscription and one with no special condition — got \
         {outcomes:?}"
    );
}
