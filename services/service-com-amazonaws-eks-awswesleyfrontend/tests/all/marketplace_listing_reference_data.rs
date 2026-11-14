// SPDX-License-Identifier: CC0-1.0

//! The AWS Marketplace reference data `DescribeAddonVersions` publishes for the
//! add-ons a caller subscribes to, driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts.
//!
//! The RULE both suites assert: for an add-on sold through AWS Marketplace, the
//! catalog publishes the LISTING's own data — the seller's published name as
//! `publisher`, and that listing's Marketplace product identifier and product
//! address as `marketplaceInformation.productId` / `productUrl` — none of which
//! is computed from the name the add-on is installed by, and none of which is
//! computed from the others. An add-on Amazon EKS publishes itself is on no
//! listing and reports no `marketplaceInformation` at all.
//!
//! Both suites walk EVERY Marketplace add-on the catalog publishes, read from the
//! publication itself, so the rows differ along what the rule generalises over:
//! which seller publishes the add-on (single-word, multi-word and dotted
//! published names), whether the add-on is a vendor's own product or its rebuild
//! of an Amazon EKS-managed add-on, and which form its listing is published at
//! (a product view, or the stock-keeping unit it is sold under).

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    ADDON_OWNER_MARKETPLACE, ADDON_PUBLISHER_EKS, MARKETPLACE_LISTING_ADDRESS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, Answer};

const REGION: &str = "us-west-2";

/// The published add-on catalog, narrowed by `query`.
async fn describe_addon_versions(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = format!("/addons/supported-versions?{query}");
    let answer = call(router, REGION, "GET", &uri, "").await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeAddonVersions({query}): expected HTTP 200, got {answer:?}"
    );
    answer
}

/// The `addons` of one listing.
fn addons(answer: &Answer) -> Vec<serde_json::Value> {
    answer.body["addons"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an addons list, got {:?}", answer.body))
        .clone()
}

/// A string member of a JSON object, or the empty string when it carries none.
fn text(value: &serde_json::Value, member: &str) -> String {
    value[member].as_str().unwrap_or_default().to_string()
}

/// The whole catalog, keyed by add-on name.
async fn catalog(router: &MultiServiceRouter) -> BTreeMap<String, serde_json::Value> {
    let answer = describe_addon_versions(router, "").await;
    let rows: BTreeMap<String, serde_json::Value> = addons(&answer)
        .into_iter()
        .map(|addon| (text(&addon, "addonName"), addon))
        .collect();
    assert!(
        !rows.is_empty(),
        "setup: the catalog must publish add-ons, got {:?}",
        answer.body
    );
    rows
}

/// The vendor token an add-on's `<vendor>_<product>` name starts with, for an
/// add-on sold through AWS Marketplace; the empty string for one published under
/// a bare name.
fn vendor_token(addon_name: &str) -> String {
    addon_name
        .split_once('_')
        .map(|(vendor, _)| vendor.to_string())
        .unwrap_or_default()
}

/// A published seller name as its letters and digits alone, so "EG Innovations"
/// and "Solo.io" compare against the `eg-innovations` and `solo-io` tokens the
/// add-on names carry.
fn alphanumeric(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether `product_id` is the form an AWS Marketplace product identifier takes:
/// `prod-` and thirteen lower-case alphanumerics.
///
/// Source: the product identifiers the AWS Marketplace APIs publish, whose
/// `prod-<id>` form every listing's product id takes.
pub(crate) fn is_marketplace_product_id(product_id: &str) -> bool {
    let Some(identifier) = product_id.strip_prefix("prod-") else {
        return false;
    };
    identifier.len() == 13
        && identifier
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
}

/// The path one listing is published at under [`MARKETPLACE_LISTING_ADDRESS`]:
/// the product view the listing is published at (`/prodview-<id>`), or the
/// stock-keeping unit it is sold under (`?sku=<sku>`) — `None` for an address
/// that leads to no AWS Marketplace listing at all.
pub(crate) fn marketplace_listing_path(product_url: &str) -> Option<&str> {
    let path = product_url.strip_prefix(MARKETPLACE_LISTING_ADDRESS)?;
    (path.len() > 1 && (path.starts_with('/') || path.starts_with('?'))).then_some(path)
}

// ---------------------------------------------------------------------------
// RULE: a Marketplace add-on's publisher and marketplaceInformation are the
// LISTING's own published data, carried per listing by the catalog — the seller's
// published name, that listing's Marketplace product identifier and that
// listing's product address — and an add-on Amazon EKS publishes itself carries
// no listing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_marketplace_addon_publishes_its_sellers_own_listing() {
    let router = emulator();
    let catalog = catalog(&router).await;

    let marketplace: Vec<(&String, &serde_json::Value)> = catalog
        .iter()
        .filter(|(_, addon)| text(addon, "owner") == ADDON_OWNER_MARKETPLACE)
        .collect();
    assert!(
        marketplace.len() > 1,
        "setup: the catalog must publish several Marketplace add-ons, got {:?}",
        marketplace.len()
    );

    let mut product_ids: BTreeSet<String> = BTreeSet::new();
    let mut product_urls: BTreeSet<String> = BTreeSet::new();
    for (addon_name, addon) in &marketplace {
        let case = addon_name.as_str();

        // The seller's own published name: it names the same seller the add-on's
        // vendor token does, and it is NOT that lower-case token, which is what a
        // name taken off the add-on's name would be.
        let publisher = text(addon, "publisher");
        let token = vendor_token(case);
        assert!(
            !publisher.is_empty(),
            "case {case}: expected the seller's published name, got {:?}",
            addon["publisher"]
        );
        assert!(
            alphanumeric(&token).starts_with(&alphanumeric(&publisher)),
            "case {case}: the published name must name the seller {token:?}, got {publisher:?}"
        );
        assert_ne!(
            publisher, token,
            "case {case}: expected the seller's published name, got the add-on's name token"
        );

        // The listing it is subscribed through: the Marketplace product
        // identifier and the address that product is published at.
        let listing = addon
            .get("marketplaceInformation")
            .filter(|listing| !listing.is_null())
            .unwrap_or_else(|| {
                panic!("case {case}: a Marketplace add-on reports its listing, got {addon:?}")
            });
        let product_id = text(listing, "productId");
        let product_url = text(listing, "productUrl");
        assert!(
            is_marketplace_product_id(&product_id),
            "case {case}: expected a Marketplace product identifier, got {product_id:?}"
        );
        let listing_path = marketplace_listing_path(&product_url).unwrap_or_else(|| {
            panic!(
                "case {case}: expected a listing published at {MARKETPLACE_LISTING_ADDRESS}, \
                 got {product_url:?}"
            )
        });

        // Two pieces of one listing's published data, so neither is computed from
        // the other: the address does not carry the product identifier.
        let identifier = product_id.trim_start_matches("prod-");
        assert!(
            !listing_path.contains(identifier),
            "case {case}: the listing's address must not be built from its product id \
             {product_id:?}, got {product_url:?}"
        );
        // Nor from the add-on's own name, or the name of the software a rebuild
        // rebuilds.
        for name in [case, token.as_str()] {
            assert!(
                !product_url.contains(name) && !product_id.contains(name),
                "case {case}: the listing is the seller's data, not the add-on's name \
                 {name:?}, got {product_id:?} and {product_url:?}"
            );
        }

        // One listing per add-on: no two add-ons are sold as one product.
        assert!(
            product_ids.insert(product_id.clone()),
            "case {case}: two add-ons report one product id, got {product_id:?} twice"
        );
        assert!(
            product_urls.insert(product_url.clone()),
            "case {case}: two add-ons report one product address, got {product_url:?} twice"
        );

        // The same listing whichever filter path reaches the add-on, and the same
        // on a second read: the catalog is a publication.
        let by_name = describe_addon_versions(&router, &format!("addonName={case}")).await;
        let named = addons(&by_name);
        assert_eq!(
            named.len(),
            1,
            "case {case}: a name the catalog publishes answers exactly that add-on, got {:?}",
            by_name.body["addons"]
        );
        for member in ["publisher", "owner", "marketplaceInformation"] {
            assert_eq!(
                named[0][member], addon[member],
                "case {case}: the published {member} must not move between reads, got {:?}",
                named[0][member]
            );
        }
    }

    // An add-on Amazon EKS publishes itself is sold on no listing at all.
    for (addon_name, addon) in &catalog {
        if text(addon, "owner") == ADDON_OWNER_MARKETPLACE {
            continue;
        }
        assert_eq!(
            text(addon, "publisher"),
            ADDON_PUBLISHER_EKS,
            "case {addon_name}: an add-on Amazon EKS publishes is published by \
             {ADDON_PUBLISHER_EKS:?}, got {:?}",
            addon["publisher"]
        );
        assert!(
            addon
                .get("marketplaceInformation")
                .is_none_or(serde_json::Value::is_null),
            "case {addon_name}: an add-on that is not sold on Marketplace has no listing, got {:?}",
            addon["marketplaceInformation"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the seller's published name is the value `DescribeAddonVersions$publishers`
// filters on, so a client that discovers an add-on by its seller's published name
// finds it — and the lower-case token the add-on's name starts with is not a
// publisher anything is published by.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_publishers_filter_matches_the_sellers_published_name() {
    let router = emulator();
    let catalog = catalog(&router).await;

    // Every seller the catalog publishes a Marketplace add-on for, with the
    // add-ons it publishes: the rows are whatever the publication offers.
    let mut sellers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (addon_name, addon) in &catalog {
        if text(addon, "owner") != ADDON_OWNER_MARKETPLACE {
            continue;
        }
        sellers
            .entry(text(addon, "publisher"))
            .or_default()
            .push(addon_name.clone());
    }
    assert!(
        sellers.len() > 1,
        "setup: the catalog must publish add-ons of several sellers, got {sellers:?}"
    );

    for (publisher, published) in &sellers {
        let case = publisher.as_str();
        // A published name may carry a space, which travels percent-encoded.
        let filter = publisher.replace(' ', "%20");
        let answer = describe_addon_versions(&router, &format!("publishers={filter}")).await;
        let kept: Vec<String> = addons(&answer)
            .iter()
            .map(|addon| text(addon, "addonName"))
            .collect();
        assert_eq!(
            &kept, published,
            "case {case}: the seller's published name keeps exactly its add-ons, got {kept:?}"
        );

        // The lower-case token the add-on names start with is not what the add-on
        // is published by, so it keeps nothing.
        let token = vendor_token(&published[0]);
        if token != *publisher {
            let by_token = describe_addon_versions(&router, &format!("publishers={token}")).await;
            assert!(
                addons(&by_token).is_empty(),
                "case {case}: {token:?} publishes nothing, got {:?}",
                by_token.body["addons"]
            );
        }
    }

    // And the sellers' add-ons are exactly the Marketplace-owned catalog.
    let by_owner =
        describe_addon_versions(&router, &format!("owners={ADDON_OWNER_MARKETPLACE}")).await;
    let marketplace_owned: BTreeSet<String> = addons(&by_owner)
        .iter()
        .map(|addon| text(addon, "addonName"))
        .collect();
    let by_seller: BTreeSet<String> = sellers.values().flatten().cloned().collect();
    assert_eq!(
        marketplace_owned, by_seller,
        "the Marketplace catalogue is what its sellers publish, got {marketplace_owned:?}"
    );
}
