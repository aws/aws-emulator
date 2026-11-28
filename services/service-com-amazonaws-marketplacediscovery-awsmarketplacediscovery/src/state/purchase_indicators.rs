// SPDX-License-Identifier: CC0-1.0

//! The purchase conditions a listing or a product reports.
//!
//! `GetListingOutput$purchaseIndicators` is "[i]ndicators describing purchase
//! characteristics of this listing, such as whether you can subscribe to it or use it
//! without a subscription. The list is empty when no special purchase conditions apply"
//! (`smithy_query(query="doc",shape="GetListingOutput",member="purchaseIndicators")`), and
//! `GetProductOutput` carries the same member for a product.
//!
//! So an indicator RESTATES something about the purchase options the record is sold through
//! rather than being a fact stored beside them: it is derived from the OFFERS the record is
//! bought under, exactly as the record's pricing models and badges are
//! ([`ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState::listing_pricing_models`]),
//! so a record can never claim a purchase condition the offers a buyer can actually accept
//! contradict.
//!
//! The two conditions the model declares (`PurchaseIndicatorType`), and what each says about
//! those offers:
//!
//! * `AVAILABLE_FOR_USE_WITHOUT_SUBSCRIPTION` — "the product can be used without a
//!   subscription": some offer still on sale is priced under a model that charges nothing for
//!   the software itself, which is `FREE` (no charge) or `BYOL` (the buyer already holds the
//!   licence) of the `PricingModelType` enum.
//! * `NOT_AVAILABLE_FOR_SUBSCRIPTION` — "the product is not available for new
//!   subscriptions": no offer of the record is on sale at all, either because every one of
//!   them has expired or because the record has none.
//!
//! A record sold only under a paid, still-available offer reports NEITHER, which is the empty
//! list the member documents.

use super::*;

use crate::ops::types::PurchaseIndicator;

/// The pricing models under which the software itself is not charged for, so a buyer may use
/// it without taking out a subscription.
///
/// Source: the `PricingModelType` enum
/// (`smithy_query(query="shape",shape="PricingModelType")`, generated as
/// `PRICING_MODEL_TYPE_VALUES`) — `FREE` charges nothing and `BYOL` is "bring your own
/// licence", where the entitlement comes from a licence the buyer already holds; `CONTRACT`
/// and `USAGE` are both charged.
const UNSUBSCRIBED_USE_PRICING_MODELS: &[&str] = &["BYOL", "FREE"];

/// The indicator for a record a buyer may use without subscribing.
const AVAILABLE_FOR_USE_WITHOUT_SUBSCRIPTION: &str = "AVAILABLE_FOR_USE_WITHOUT_SUBSCRIPTION";

/// The indicator for a record no buyer can subscribe to any more.
const NOT_AVAILABLE_FOR_SUBSCRIPTION: &str = "NOT_AVAILABLE_FOR_SUBSCRIPTION";

impl ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState {
    /// The purchase conditions `listing` reports, derived from the offers it associates.
    #[must_use]
    pub fn listing_purchase_indicators(
        &self,
        listing: &Listing,
        now_secs: i64,
    ) -> Vec<PurchaseIndicator> {
        let offers: Vec<&Offer> = listing
            .offer_ids
            .iter()
            .filter_map(|offer_id| self.offers.get(offer_id))
            .collect();
        purchase_indicators(&offers, now_secs)
    }

    /// The purchase conditions the product `product_id` reports, derived from the offers that
    /// cover it — which is how a buyer purchases a product, so the same rule reads the same
    /// records as for the listing that names it.
    #[must_use]
    pub fn product_purchase_indicators(
        &self,
        product_id: &str,
        now_secs: i64,
    ) -> Vec<PurchaseIndicator> {
        let offers: Vec<&Offer> = self
            .offers
            .values()
            .filter(|offer| offer.product_ids.iter().any(|covered| covered == product_id))
            .collect();
        purchase_indicators(&offers, now_secs)
    }
}

/// The purchase conditions a record sold through `offers` reports at `now_secs`.
///
/// Whether an offer is still on sale is the one derivation this service already makes from an
/// offer's expiration ([`availability_status`]), so a purchase indicator and a purchase
/// option's `availabilityStatus` cannot disagree about which offers a buyer can accept.
fn purchase_indicators(offers: &[&Offer], now_secs: i64) -> Vec<PurchaseIndicator> {
    let available: Vec<&&Offer> = offers
        .iter()
        .filter(|offer| {
            availability_status(offer.expiration_time, now_secs) == AVAILABILITY_STATUS_AVAILABLE
        })
        .collect();
    let indicator = if available.is_empty() {
        NOT_AVAILABLE_FOR_SUBSCRIPTION
    } else if available.iter().any(|offer| {
        UNSUBSCRIBED_USE_PRICING_MODELS.contains(&offer.pricing_model.pricing_model_type.as_str())
    }) {
        AVAILABLE_FOR_USE_WITHOUT_SUBSCRIPTION
    } else {
        // Nothing special applies to a record sold only under a paid offer that is still on
        // sale, which is the empty list the member documents.
        return Vec::new();
    };
    vec![PurchaseIndicator {
        indicator_type: indicator.to_string(),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ops::types::PURCHASE_INDICATOR_TYPE_VALUES;

    /// One row of the table below: the case, the offers' `(pricingModelType,
    /// expirationTime)` pairs, and the indicator types the rule expects.
    type Row = (&'static str, Vec<(&'static str, Option<i64>)>, Vec<&'static str>);

    /// RULE: the indicators a record reports follow the offers it is sold through, over a
    /// table spanning the forms a record's offer set takes — none at all, one expired, one
    /// still on sale under each pricing model the enum declares, and several at once.
    #[test]
    fn the_indicators_follow_the_offers_a_record_is_sold_through() {
        let now = 1_700_000_000_i64;
        let live = Some(now + 30 * DAY);
        let expired = Some(now - DAY);

        let cases: Vec<Row> = vec![
            ("no offer at all", vec![], vec![NOT_AVAILABLE_FOR_SUBSCRIPTION]),
            (
                "one expired offer",
                vec![("CONTRACT", expired)],
                vec![NOT_AVAILABLE_FOR_SUBSCRIPTION],
            ),
            (
                "every offer expired",
                vec![("CONTRACT", expired), ("FREE", expired)],
                vec![NOT_AVAILABLE_FOR_SUBSCRIPTION],
            ),
            ("one paid offer on sale", vec![("CONTRACT", live)], vec![]),
            ("usage-priced offer on sale", vec![("USAGE", live)], vec![]),
            (
                "a free offer on sale",
                vec![("FREE", live)],
                vec![AVAILABLE_FOR_USE_WITHOUT_SUBSCRIPTION],
            ),
            (
                "a bring-your-own-licence offer on sale",
                vec![("BYOL", live)],
                vec![AVAILABLE_FOR_USE_WITHOUT_SUBSCRIPTION],
            ),
            (
                "a paid offer beside a free one, both on sale",
                vec![("CONTRACT", live), ("FREE", live)],
                vec![AVAILABLE_FOR_USE_WITHOUT_SUBSCRIPTION],
            ),
            (
                "a free offer that has expired beside a paid one on sale",
                vec![("FREE", expired), ("CONTRACT", live)],
                vec![],
            ),
            (
                "an offer that never expires",
                vec![("BYOL", None)],
                vec![AVAILABLE_FOR_USE_WITHOUT_SUBSCRIPTION],
            ),
        ];

        for (case, rows, expected) in cases {
            let offers: Vec<Offer> = rows
                .iter()
                .enumerate()
                .map(|(index, (model, expiration))| Offer {
                    offer_id: format!("o-{index}"),
                    expiration_time: *expiration,
                    pricing_model: PricingModel {
                        pricing_model_type: (*model).to_string(),
                        display_name: (*model).to_string(),
                    },
                    ..Offer::default()
                })
                .collect();
            let borrowed: Vec<&Offer> = offers.iter().collect();
            let reported: Vec<String> = purchase_indicators(&borrowed, now)
                .into_iter()
                .map(|indicator| indicator.indicator_type)
                .collect();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {expected:?}, got {reported:?}"
            );
            for indicator in &reported {
                assert!(
                    PURCHASE_INDICATOR_TYPE_VALUES.contains(&indicator.as_str()),
                    "case {case:?}: {indicator:?} is outside PurchaseIndicatorType"
                );
            }
        }
    }
}
