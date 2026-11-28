// SPDX-License-Identifier: CC0-1.0

//! The Amazon Web Services Marketplace SELLERS the product catalogue publishes, end to
//! end over the wire.
//!
//! The rule asserted here: `DescribeAllManagedProducts` "provides high-level
//! information for the Amazon Web Services Managed Rules rule groups and Amazon Web
//! Services Marketplace managed rule groups" (its own doc), so the report is the UNION
//! of both vendor families — every Marketplace seller's listings, in published order
//! and ahead of the Amazon Web Services Managed Rules family — and a listing is
//! reported as its own SELLER publishes it: under that seller's `VendorName`, with the
//! identifier the seller assigns the product, a `ProductLink` to the seller's product
//! page on the Marketplace, and the ARN of the topic that seller announces its changes
//! on, in the SELLER's own account. One topic per seller, so no two sellers share one
//! and none of them is the Amazon Web Services Managed Rules topic. A product is a
//! vendor's publication and a `Scope` is only the namespace a request is made in, so
//! the same products are reported in either.
//!
//! The table is every SELLER the catalogue publishes a listing for — more than one, so
//! nothing here can hold for only the first — crossed with both declared scopes, and
//! the Amazon Web Services Managed Rules family is the contrast row: its products carry
//! no `ProductLink` at all and announce their changes on that family's own topic. The
//! sibling suites assert what one descriptor CARRIES
//! ([`crate::managed_product_catalog_data`]), the SHAPE of the whole report
//! ([`crate::managed_products`]) and the per-vendor report
//! ([`crate::managed_products_by_vendor`]); what this one adds is that the Marketplace
//! family spans several sellers and is published ahead of the other family.
//!
//! Every expectation is composed FROM the catalogue rather than written as a literal,
//! and every call goes through the router's front door (the `Waf` harness in
//! [`crate::web_acl_lifecycle`]), the same door an SDK uses.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    managed_products_of_vendor, MarketplaceSeller, AWS_MANAGED_RULE_GROUP_VENDOR,
    MANAGED_RULE_GROUP_CATALOGUE, MARKETPLACE_MANAGED_PRODUCT_CATALOGUE,
    MARKETPLACE_PRODUCT_PAGE_ADDRESS,
};

use crate::managed_products::{product_of, reported};
use crate::web_acl_lifecycle::Waf;

/// The two namespaces a request can be made in (`Scope`'s declared values).
const SCOPES: [&str; 2] = ["REGIONAL", "CLOUDFRONT"];

/// Every seller the catalogue publishes a listing for, in published order and without
/// repeats: the rows of every table here.
fn sellers() -> Vec<&'static MarketplaceSeller> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    MARKETPLACE_MANAGED_PRODUCT_CATALOGUE
        .iter()
        .filter(|listing| seen.insert(listing.seller.vendor_name))
        .map(|listing| listing.seller)
        .collect()
}

/// Every descriptor `DescribeAllManagedProducts` reports in `scope`.
async fn all_products(waf: &Waf, scope: &str) -> Vec<Value> {
    reported(
        &waf.ok("DescribeAllManagedProducts", json!({ "Scope": scope }))
            .await,
    )
}

/// The whole Marketplace family is published AHEAD of the Amazon Web Services Managed
/// Rules family, in the catalogue's own order, in both scopes.
///
/// One row per published product of either family — the report is the union of the two
/// — crossed with the two scopes.
#[tokio::test]
async fn the_marketplace_family_is_published_before_the_aws_managed_rules_family() {
    assert!(
        sellers().len() > 1 && !MANAGED_RULE_GROUP_CATALOGUE.is_empty(),
        "the catalogue must publish listings of more than one seller beside the Amazon \
         Web Services Managed Rules family for this table to span the class, got {} \
         seller(s)",
        sellers().len()
    );

    let waf = Waf::new();
    for scope in SCOPES {
        let all = all_products(&waf, scope).await;
        let vendors: Vec<&str> = all
            .iter()
            .map(|product| product["VendorName"].as_str().unwrap_or_default())
            .collect();
        let expected_order: Vec<&str> = MARKETPLACE_MANAGED_PRODUCT_CATALOGUE
            .iter()
            .map(|listing| listing.seller.vendor_name)
            .chain(MANAGED_RULE_GROUP_CATALOGUE.iter().map(|group| group.vendor_name))
            .collect();
        assert_eq!(
            vendors, expected_order,
            "case {scope:?}: expected one descriptor per published product, the \
             Marketplace sellers' listings in published order and then the Amazon Web \
             Services Managed Rules family, got {vendors:?}"
        );
    }

    // A product is a vendor's publication, so the two namespaces report the same list.
    let regional = all_products(&waf, SCOPES[0]).await;
    let cloudfront = all_products(&waf, SCOPES[1]).await;
    assert_eq!(
        regional, cloudfront,
        "the same products are published in either namespace, got {} and {} entries",
        regional.len(),
        cloudfront.len()
    );
}

/// Every seller's listings are reported under that seller's own identity: its vendor
/// name, the identifier it assigns each product, a link to its product page on the
/// Marketplace, and ONE notification topic of its own, in its own account — and the
/// same listings answer `DescribeManagedProductsByVendor` for that seller.
///
/// The rows are every seller the catalogue publishes a listing for, crossed with both
/// scopes; the assertions after the loop are what holds ACROSS sellers (a topic per
/// seller, none of them shared and none of them the Amazon Web Services Managed Rules
/// one), and the Amazon Web Services Managed Rules family is the contrast.
#[tokio::test]
async fn each_marketplace_seller_publishes_its_own_listings_under_its_own_identity() {
    let waf = Waf::new();
    let mut topics: BTreeMap<&str, String> = BTreeMap::new();

    for scope in SCOPES {
        let all = all_products(&waf, scope).await;
        for seller in sellers() {
            let vendor = seller.vendor_name;
            let case = format!("{scope}/{vendor}");
            let listings: Vec<_> = MARKETPLACE_MANAGED_PRODUCT_CATALOGUE
                .iter()
                .filter(|listing| listing.seller.vendor_name == vendor)
                .collect();
            assert!(
                !listings.is_empty(),
                "case {case:?}: a seller of this table publishes at least one listing"
            );

            // What the whole-catalogue report carries for this seller is what the
            // per-vendor report answers, and what the catalogue publishes for it.
            let reported_of_vendor: Vec<Value> = all
                .iter()
                .filter(|product| product["VendorName"] == json!(vendor))
                .cloned()
                .collect();
            let by_vendor = reported(
                &waf.ok(
                    "DescribeManagedProductsByVendor",
                    json!({ "Scope": scope, "VendorName": vendor }),
                )
                .await,
            );
            assert_eq!(
                reported_of_vendor.len(),
                listings.len(),
                "case {case:?}: expected this seller's {} published listing(s), got {}: \
                 {reported_of_vendor:?}",
                listings.len(),
                reported_of_vendor.len()
            );
            assert_eq!(
                by_vendor, reported_of_vendor,
                "case {case:?}: the per-vendor report is this seller's slice of the \
                 catalogue, got {by_vendor:?}"
            );

            for listing in listings {
                let name = listing.managed_rule_set_name;
                let case = format!("{case}/{name}");
                let Some(product) = product_of(&all, vendor, name) else {
                    panic!("case {case:?}: the seller's listing must be reported");
                };
                // The identifier its publisher assigns the product, which is the
                // product's own and not the rule-set name a caller writes in a
                // `ManagedRuleGroupStatement`.
                let identifier = product["ProductId"].as_str().unwrap_or_default();
                assert_ne!(
                    identifier, name,
                    "case {case:?}: a product identifier is the product's own, got \
                     {product}"
                );
                if let Some(published) = listing.product_id {
                    assert_eq!(
                        identifier, published,
                        "case {case:?}: expected the identifier this product is \
                         published under, got {product}"
                    );
                }
                // "For Amazon Web Services Marketplace managed rule groups only, the
                // link to the rule group product page" (`ProductLink`).
                let link = product["ProductLink"].as_str().unwrap_or_default();
                assert!(
                    link.contains(MARKETPLACE_PRODUCT_PAGE_ADDRESS),
                    "case {case:?}: expected a link to this listing's page on the \
                     Marketplace, got {product}"
                );
                // The topic the SELLER announces its changes on, in the seller's own
                // account.
                let topic = product["SnsTopicArn"].as_str().unwrap_or_default();
                assert_eq!(
                    topic,
                    seller.notification_topic_arn(),
                    "case {case:?}: expected this seller's own notification topic, got \
                     {product}"
                );
                if let Some(account) = seller.notification_topic_account {
                    assert!(
                        topic.contains(&format!(":{account}:"))
                            && !topic.contains(&format!(":{}:", ::engine::emulator_account_id())),
                        "case {case:?}: a seller's topic lives in the seller's own \
                         account, got {topic:?}"
                    );
                }
                let previous = topics.insert(vendor, topic.to_string());
                assert!(
                    previous.is_none_or(|known| known == topic),
                    "case {case:?}: a seller announces every change it publishes on ONE \
                     topic, got {topic:?}"
                );
            }
        }

        // The contrast: the Amazon Web Services Managed Rules family carries no
        // product link at all, and announces on its own family's topic rather than any
        // seller's.
        for group in MANAGED_RULE_GROUP_CATALOGUE {
            let case = format!("{scope}/{}/{}", group.vendor_name, group.name);
            let Some(product) = product_of(&all, group.vendor_name, group.name) else {
                panic!("case {case:?}: the published rule set must be reported");
            };
            assert_eq!(
                (product["ProductLink"].is_null(), product["VendorName"].as_str()),
                (true, Some(AWS_MANAGED_RULE_GROUP_VENDOR)),
                "case {case:?}: a product link is for a Marketplace managed rule group \
                 only, got {product}"
            );
            let topic = product["SnsTopicArn"].as_str().unwrap_or_default();
            assert!(
                topics.values().all(|seller_topic| seller_topic != topic),
                "case {case:?}: the Amazon Web Services Managed Rules topic is not any \
                 seller's, got {topic:?}"
            );
        }
    }

    // One topic per seller, and no two sellers sharing one.
    let distinct: BTreeSet<&String> = topics.values().collect();
    assert_eq!(
        distinct.len(),
        topics.len(),
        "each seller announces its changes on its own topic, got {topics:?}"
    );
    assert_eq!(
        topics.len(),
        sellers().len(),
        "every seller of the catalogue must have been reported, got {topics:?}"
    );
}

/// The catalogue's own view of each seller's listings is the one both describes
/// project: `managed_products_of_vendor` answers exactly what the wire reports for
/// that seller, for every seller.
///
/// The rows are every seller, crossed with both scopes.
#[tokio::test]
async fn the_catalogue_and_the_wire_agree_on_every_sellers_listings() {
    let waf = Waf::new();
    for scope in SCOPES {
        let all = all_products(&waf, scope).await;
        for seller in sellers() {
            let vendor = seller.vendor_name;
            let case = format!("{scope}/{vendor}");
            let catalogued: Vec<String> = managed_products_of_vendor(vendor)
                .iter()
                .map(|product| product.managed_rule_set_name.clone().unwrap_or_default())
                .collect();
            let on_the_wire: Vec<String> = all
                .iter()
                .filter(|product| product["VendorName"] == json!(vendor))
                .map(|product| {
                    product["ManagedRuleSetName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            assert_eq!(
                on_the_wire, catalogued,
                "case {case:?}: the report is the catalogue's own listing of this \
                 seller, got {on_the_wire:?}"
            );
        }
    }
}
