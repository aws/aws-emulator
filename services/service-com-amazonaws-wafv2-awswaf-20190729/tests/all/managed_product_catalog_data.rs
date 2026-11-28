// SPDX-License-Identifier: CC0-1.0

//! What a managed-product descriptor CARRIES, end to end over the wire.
//!
//! The rule asserted here: a managed product is a vendor's publication read out of
//! the service's own fixed catalogue, so every member a descriptor carries is the
//! PUBLISHER's — a `ProductTitle` for every product and its publisher's own
//! `ProductDescription` wherever the catalogue cites one — and each member that names
//! something the publishing VENDOR owns (the product's
//! identifier, the link to its product page, the ARN of the topic the vendor
//! announces its changes on) is reported exactly as the catalogue publishes it,
//! never rewritten through the local-account/URL normalisation this emulator applies
//! to resources a CALLER owns.
//!
//! The sibling suite [`crate::managed_products`] asserts the SHAPE of the report —
//! both vendor families present, one descriptor per published product, identifiers
//! unique and stable, one notification topic per vendor, the same report from either
//! describe — and its helpers are reused here. What these tables add is the
//! published CONTENT of the members above, over every product of either family in
//! both declared scopes. Members and their constraints are the model's
//! (`smithy_query(query="shape",shape="ManagedProductDescriptor")`: `ProductTitle`
//! and `ProductDescription` `@length min 1` + `@pattern \S`, `SnsTopicArn` a
//! `ResourceArn` of `@length 20..2048`, `ProductId` `@length 1..128` + `@pattern
//! \S`).
//!
//! Every call goes through the router's front door with the harness in
//! [`crate::web_acl_lifecycle`], so what these tests observe is what an SDK
//! observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    MANAGED_RULE_GROUP_CATALOGUE, MARKETPLACE_MANAGED_PRODUCT_CATALOGUE,
    MARKETPLACE_PRODUCT_PAGE_ADDRESS,
};

use crate::managed_products::{product_of, reported};
use crate::web_acl_lifecycle::Waf;

/// The two namespaces a request can be made in (`Scope`'s declared values), which
/// every table here crosses: a product is a vendor's publication, so the catalogue
/// reports the same descriptors in either.
const SCOPES: [&str; 2] = ["REGIONAL", "CLOUDFRONT"];

/// Every descriptor `DescribeAllManagedProducts` reports in `scope`.
async fn all_products(waf: &Waf, scope: &str) -> Vec<Value> {
    reported(
        &waf.ok("DescribeAllManagedProducts", json!({ "Scope": scope }))
            .await,
    )
}

/// Every `(VendorName, ManagedRuleSetName)` pair the catalogue publishes, of either
/// vendor family: the rows the first table walks.
fn published_pairs() -> Vec<(&'static str, &'static str)> {
    MARKETPLACE_MANAGED_PRODUCT_CATALOGUE
        .iter()
        .map(|listing| (listing.seller.vendor_name, listing.managed_rule_set_name))
        .chain(
            MANAGED_RULE_GROUP_CATALOGUE
                .iter()
                .map(|group| (group.vendor_name, group.name)),
        )
        .collect()
}

/// The text a descriptor's display member carries, asserted against the model's
/// constraints on `ProductTitle`/`ProductDescription`: present, non-empty, and
/// carrying a non-whitespace character (`@length min 1`, `@pattern \S`).
fn assert_published_text(product: &Value, member: &str, case: &str) {
    let Some(text) = product[member].as_str() else {
        panic!("case {case:?}: every descriptor carries {member}, got {product}");
    };
    assert!(
        !text.is_empty() && text.trim() == text && text.chars().any(|c| !c.is_whitespace()),
        "case {case:?}: {member} must be the publisher's own text, got {text:?}"
    );
}

/// Every product of either family reports its publisher's display text — a
/// `ProductTitle` for every product, and the `ProductDescription` its publisher
/// states where the catalogue cites one — in both scopes, because a describe reports
/// what a product's publisher states about it and nothing this emulator worded.
///
/// One row per published product of either vendor family, crossed with the two
/// scopes: the rows span the Amazon Web Services Managed Rules family and the
/// Marketplace family, versioned and unversioned products, advanced and ordinary
/// ones, and listings whose published summary the catalogue cites as well as ones it
/// cannot.
#[tokio::test]
async fn every_managed_product_reports_its_publishers_title_and_description() {
    let pairs = published_pairs();
    assert!(
        pairs.len() > 1
            && !MARKETPLACE_MANAGED_PRODUCT_CATALOGUE.is_empty()
            && !MANAGED_RULE_GROUP_CATALOGUE.is_empty(),
        "the catalogue must publish products of both families for this table to span \
         the class, got {} rows",
        pairs.len()
    );

    let waf = Waf::new();
    for scope in SCOPES {
        let all = all_products(&waf, scope).await;
        for (vendor, name) in &pairs {
            let case = format!("{scope}/{vendor}/{name}");
            let Some(product) = product_of(&all, vendor, name) else {
                panic!("case {case:?}: the published product must be reported");
            };
            assert_published_text(product, "ProductTitle", &case);
            // A summary is published text too, so where the descriptor carries one it
            // is the publisher's; a product whose summary no source states carries no
            // `ProductDescription` at all rather than text nobody published.
            if !product["ProductDescription"].is_null() {
                assert_published_text(product, "ProductDescription", &case);
            }
        }
    }

    // Every product of the Amazon Web Services Managed Rules family is published with
    // a summary, so none of those rows may be the omitted kind.
    let all = all_products(&waf, SCOPES[0]).await;
    for group in MANAGED_RULE_GROUP_CATALOGUE {
        let case = format!("{}/{}", group.vendor_name, group.name);
        let Some(product) = product_of(&all, group.vendor_name, group.name) else {
            panic!("case {case:?}: the published rule set must be reported");
        };
        assert_published_text(product, "ProductDescription", &case);
    }
}

/// A Marketplace product's `ProductLink` is the address its product page is published
/// under — scheme and host exactly as published, not normalised and not an address
/// this emulator serves — and a product of the Amazon Web Services Managed Rules
/// family carries no link at all, the member being "for Amazon Web Services
/// Marketplace managed rule groups only".
///
/// One row per published product of either family, in both scopes: the Marketplace
/// rows assert the published link, the Amazon Web Services Managed Rules rows assert
/// its absence. The Marketplace rows span the published forms of such a link, which
/// differ in scheme and in the segment the page is addressed by.
#[tokio::test]
async fn a_marketplace_product_link_is_published_verbatim() {
    // The Marketplace's own product-page address, as the catalogue states it: a
    // product page is the Marketplace's own, so this is reference data rather than a
    // URL this emulator serves.
    assert_eq!(
        MARKETPLACE_PRODUCT_PAGE_ADDRESS, "aws.amazon.com/marketplace/pp",
        "the catalogue publishes the Marketplace product-page address as AWS does, \
         got {MARKETPLACE_PRODUCT_PAGE_ADDRESS:?}"
    );

    let waf = Waf::new();
    for scope in SCOPES {
        let all = all_products(&waf, scope).await;

        for listing in MARKETPLACE_MANAGED_PRODUCT_CATALOGUE {
            let vendor = listing.seller.vendor_name;
            let name = listing.managed_rule_set_name;
            let case = format!("{scope}/{vendor}/{name}");
            let Some(product) = product_of(&all, vendor, name) else {
                panic!("case {case:?}: the seller's listing must be reported");
            };
            let expected = listing.product_link;
            assert_eq!(
                product["ProductLink"],
                json!(expected),
                "case {case:?}: expected the published product-page link \
                 {expected:?}, got {product}"
            );
            let link = product["ProductLink"].as_str().unwrap_or_default();
            assert!(
                link.contains(MARKETPLACE_PRODUCT_PAGE_ADDRESS)
                    && (link.starts_with("http://") || link.starts_with("https://")),
                "case {case:?}: a published product-page link keeps the scheme and \
                 host it is published under, got {link:?}"
            );
            assert!(
                !link.contains("example.com") && !link.contains("127.0.0.1"),
                "case {case:?}: a vendor's product page is not an address this \
                 emulator serves, got {link:?}"
            );
        }

        for group in MANAGED_RULE_GROUP_CATALOGUE {
            let case = format!("{scope}/{}/{}", group.vendor_name, group.name);
            let Some(product) = product_of(&all, group.vendor_name, group.name) else {
                panic!("case {case:?}: the published rule set must be reported");
            };
            assert!(
                product["ProductLink"].is_null(),
                "case {case:?}: a product link is for a Marketplace managed rule \
                 group only, got {product}"
            );
        }
    }
}

/// A product's `ProductId` is the identifier its publishing vendor assigns it and
/// its `SnsTopicArn` names that vendor's own topic in that vendor's own account:
/// both are catalogue constants, reported as published rather than minted for a
/// resource of the calling account.
///
/// One row per Marketplace listing, in both scopes — the family whose publisher's
/// own identifier and account the catalogue cites — plus one row written from the
/// publisher's catalogue entry rather than composed from this crate's constants: the
/// whole descriptor of a listing, with its identifier, its product-page link in the
/// published `http` form, its display text and its seller's own notification-topic
/// account.
#[tokio::test]
async fn a_published_identifier_and_vendor_topic_are_reported_as_published() {
    assert!(
        MARKETPLACE_MANAGED_PRODUCT_CATALOGUE
            .iter()
            .any(|listing| listing.product_id.is_some()
                && listing.seller.notification_topic_account.is_some()),
        "the catalogue must cite a publisher's own identifier and account for these \
         rows to span the class"
    );

    let waf = Waf::new();
    for scope in SCOPES {
        let all = all_products(&waf, scope).await;

        for listing in MARKETPLACE_MANAGED_PRODUCT_CATALOGUE {
            let vendor = listing.seller.vendor_name;
            let name = listing.managed_rule_set_name;
            let case = format!("{scope}/{vendor}/{name}");
            let Some(product) = product_of(&all, vendor, name) else {
                panic!("case {case:?}: the seller's listing must be reported");
            };

            // The identifier its publisher states for it, reported verbatim — never
            // the rule-set name a caller writes in a `ManagedRuleGroupStatement`.
            let identifier = product["ProductId"].as_str().unwrap_or_default();
            if let Some(published) = listing.product_id {
                assert_eq!(
                    identifier, published,
                    "case {case:?}: expected the identifier this product is \
                     published under, got {identifier:?}"
                );
            }
            assert_ne!(
                identifier, name,
                "case {case:?}: a product identifier is the product's own, not its \
                 rule-set name, got {product}"
            );
            assert!(
                (1..=128).contains(&identifier.len()) && identifier.trim() == identifier,
                "case {case:?}: a ProductId is 1..128 non-blank characters, got \
                 {identifier:?}"
            );

            // `SnsTopicArn` is a `ResourceArn` (@length 20..2048): six segments, the
            // SNS service, the region the vendor publishes the topic in, the account
            // the catalogue states for that vendor, and the topic's own name.
            let expected = listing.seller.notification_topic_arn();
            assert_eq!(
                product["SnsTopicArn"],
                json!(expected),
                "case {case:?}: expected the vendor's own notification topic \
                 {expected:?}, got {product}"
            );
            let arn = product["SnsTopicArn"].as_str().unwrap_or_default();
            let segments: Vec<&str> = arn.split(':').collect();
            assert_eq!(
                segments.len(),
                6,
                "case {case:?}: expected an ARN of six segments, got {arn:?}"
            );
            assert!(
                (20..=2048).contains(&arn.len()),
                "case {case:?}: a ResourceArn is 20..2048 characters, got {arn:?}"
            );
            assert_eq!(
                segments.first().copied().unwrap_or_default(),
                "arn",
                "case {case:?}: expected an ARN, got {arn:?}"
            );
            assert_eq!(
                segments.get(2).copied().unwrap_or_default(),
                "sns",
                "case {case:?}: a change is announced on an SNS topic, got {arn:?}"
            );
            assert_eq!(
                segments.get(5).copied().unwrap_or_default(),
                listing.seller.notification_topic,
                "case {case:?}: expected the vendor's published topic name, got \
                 {arn:?}"
            );
            if let Some(account) = listing.seller.notification_topic_account {
                assert_eq!(
                    segments.get(4).copied().unwrap_or_default(),
                    account,
                    "case {case:?}: the topic lives in the publishing vendor's \
                     account, got {arn:?}"
                );
                assert_ne!(
                    segments.get(4).copied().unwrap_or_default(),
                    ::engine::emulator_account_id(),
                    "case {case:?}: a published vendor topic is not rewritten into \
                     this emulator's account, got {arn:?}"
                );
            }
        }
    }

    // The published descriptor of one Marketplace listing, written out as its
    // publisher states it: the seller's identifier, its product page's address, its
    // own display text and its seller's notification topic. Cited from the recorded
    // catalogue this crate's listings come from (the trace this suite's rule was
    // filed against, `wafv2-catalog-describe-all-managed-products`), and deliberately
    // the entry the report that motivated the fix did NOT quote.
    let published = json!({
        "VendorName": "Cyber Security Cloud Inc.",
        "ManagedRuleSetName": "CyberSecurityCloud-APIGatewayServerless-",
        "ProductId": "8bbf295b-34b3-40c0-bee7-5ba8f602705f",
        "ProductLink": "http://aws.amazon.com/marketplace/pp/B081TKTBQ8",
        "ProductTitle": "Cyber Security Cloud Managed Rules for AWS WAF -API Gateway/Serverless-",
        "ProductDescription": "The Cyber Security Cloud OWASP ruleset is designed to \
             mitigate and minimize vulnerabilities, including all those on OWASP API \
             Security/Serverless Top 10 Threats.",
        "SnsTopicArn": "arn:aws:sns:us-east-1:343255486711:CyberSecurityCloud-ManagedRule",
        "IsVersioningSupported": true,
        "IsAdvancedManagedRuleSet": false,
    });
    for scope in SCOPES {
        let all = all_products(&waf, scope).await;
        let vendor = published["VendorName"].as_str().unwrap_or_default();
        let name = published["ManagedRuleSetName"].as_str().unwrap_or_default();
        let case = format!("{scope}/{vendor}/{name}");
        let Some(product) = product_of(&all, vendor, name) else {
            panic!("case {case:?}: this listing must be reported, got {all:?}");
        };
        assert_eq!(
            product, &published,
            "case {case:?}: expected the descriptor this listing is published with, \
             got {product}"
        );
    }
}
