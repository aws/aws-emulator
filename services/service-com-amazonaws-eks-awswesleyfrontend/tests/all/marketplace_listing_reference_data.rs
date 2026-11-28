// SPDX-License-Identifier: CC0-1.0

//! The AWS Marketplace reference data `DescribeAddonVersions` publishes for the
//! add-ons a caller subscribes to, driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts.
//!
//! The RULE the first two suites assert: for an add-on sold through AWS
//! Marketplace, the catalog publishes the LISTING's own data — the seller's
//! published name as `publisher`, and that listing's Marketplace product
//! identifier as `marketplaceInformation.productId` — neither of which is
//! computed from the name the add-on is installed by, and neither of which is
//! computed from the other. An add-on Amazon EKS publishes itself is on no
//! listing and reports no `marketplaceInformation` at all.
//!
//! The RULE the remaining suites assert: `marketplaceInformation.productUrl` is
//! the AWS Marketplace product URL the catalog carries for that listing — an
//! address on the AWS Marketplace site, in one of the two forms it publishes a
//! product page under — and it is ABSENT for a listing the catalog carries no
//! published page for. It is reference data, so it does not depend on where this
//! emulator is served or on the host the reading request carried, and it is never
//! composed from either.
//!
//! Every suite walks EVERY Marketplace add-on the catalog publishes, read from the
//! publication itself, so the rows differ along what the rules generalise over:
//! which seller publishes the add-on (single-word, multi-word and dotted
//! published names), whether the add-on is a vendor's own product or its rebuild
//! of an Amazon EKS-managed add-on, which of the two published address forms its
//! product page takes (or neither), and — for the address — where this emulator is
//! served and which host the request that reads the catalog carried.

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    MarketplaceProductPage, ADDON_OWNER_MARKETPLACE, ADDON_PUBLISHER_EKS, ADDON_PUBLISHER_SAGEMAKER,
    MARKETPLACE_PRODUCT_PAGE_ADDRESS,
};
use smithy_server_runtime::{EndpointUrl, MultiServiceRouter};

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

/// Whether `product_id` is a form an AWS Marketplace product identifier takes:
/// the UUID the Marketplace catalog publishes a product id as, or the
/// `prod-<id>` form of thirteen lower-case alphanumerics.
///
/// Source: the product identifiers the AWS Marketplace catalog publishes for its
/// listings, which is what `MarketplaceInformation$productId` carries — per
/// listing, so a listing whose identifier is published as a UUID reports that
/// UUID.
pub(crate) fn is_marketplace_product_id(product_id: &str) -> bool {
    is_product_uuid(product_id) || is_product_entity_id(product_id)
}

/// The lower-case 8-4-4-4-12 hex form a Marketplace product id is published in.
fn is_product_uuid(product_id: &str) -> bool {
    let groups: Vec<&str> = product_id.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|group| {
            group
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
        })
}

/// The `prod-` and thirteen lower-case alphanumerics form the same identifier
/// also takes.
fn is_product_entity_id(product_id: &str) -> bool {
    let Some(identifier) = product_id.strip_prefix("prod-") else {
        return false;
    };
    identifier.len() == 13
        && identifier
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
}

/// The listing key an AWS Marketplace product URL addresses its product page by —
/// the `prodview-<id>` segment of the product-view form, or the `<sku>` of the
/// stock-keeping-unit form — and `None` for every address that is not a product
/// page on the AWS Marketplace site.
///
/// Source: the two forms AWS Marketplace publishes a product page under
/// (`MARKETPLACE_PRODUCT_PAGE_ADDRESS` plus `/prodview-<id>`, and that address
/// plus `?sku=<sku>`), which is what `MarketplaceInformation$productUrl` ("The
/// product URL from the Amazon Web Services Marketplace") carries.
pub(crate) fn marketplace_product_page(product_url: &str) -> Option<&str> {
    let rest = product_url.strip_prefix(MARKETPLACE_PRODUCT_PAGE_ADDRESS)?;
    let key = rest
        .strip_prefix("?sku=")
        .or_else(|| rest.strip_prefix('/').filter(|view| view.starts_with("prodview-")))?;
    let identifier = key.strip_prefix("prodview-").unwrap_or(key);
    (!identifier.is_empty()
        && identifier
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit()))
    .then_some(key)
}

// ---------------------------------------------------------------------------
// RULE: a Marketplace add-on's publisher and marketplaceInformation are the
// LISTING's own published data, carried per listing by the catalog — the seller's
// published name, that listing's Marketplace product identifier and the AWS
// Marketplace address of its product page — and an add-on Amazon EKS publishes
// itself carries no listing.
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
        // identifier, and the Marketplace address of that product's page where the
        // catalog carries one.
        let listing = addon
            .get("marketplaceInformation")
            .filter(|listing| !listing.is_null())
            .unwrap_or_else(|| {
                panic!("case {case}: a Marketplace add-on reports its listing, got {addon:?}")
            });
        let product_id = text(listing, "productId");
        assert!(
            is_marketplace_product_id(&product_id),
            "case {case}: expected a Marketplace product identifier, got {product_id:?}"
        );
        let published_url = listing.get("productUrl").map(|url| {
            url.as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: a published product URL is text, got {url:?}")
                })
                .to_string()
        });

        if let Some(product_url) = &published_url {
            let page = marketplace_product_page(product_url).unwrap_or_else(|| {
                panic!(
                    "case {case}: expected the listing's product page on the AWS Marketplace \
                     site, got {product_url:?}"
                )
            });

            // Two pieces of one listing's published data, so neither is computed
            // from the other: the address does not carry the product identifier.
            let identifier = product_id.trim_start_matches("prod-");
            assert!(
                !page.contains(identifier),
                "case {case}: the listing's address must not be built from its product id \
                 {product_id:?}, got {product_url:?}"
            );
            // Nor from the add-on's own name, or the name of the software a rebuild
            // rebuilds.
            for name in [case, token.as_str()] {
                assert!(
                    !page.contains(name) && !product_id.contains(name),
                    "case {case}: the listing is the seller's data, not the add-on's name \
                     {name:?}, got {product_id:?} and {product_url:?}"
                );
            }

            // One listing per add-on: no two add-ons are sold as one product.
            assert!(
                product_urls.insert(product_url.clone()),
                "case {case}: two add-ons report one product address, got {product_url:?} twice"
            );
        }

        assert!(
            product_ids.insert(product_id.clone()),
            "case {case}: two add-ons report one product id, got {product_id:?} twice"
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
        let publisher = text(addon, "publisher");
        assert!(
            [ADDON_PUBLISHER_EKS, ADDON_PUBLISHER_SAGEMAKER].contains(&publisher.as_str()),
            "case {addon_name}: an add-on that is not sold on Marketplace is published by Amazon \
             EKS or by the Amazon Web Services service team that builds it, got {publisher:?}"
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

// ---------------------------------------------------------------------------
// RULE: `marketplaceInformation.productUrl` is the AWS Marketplace product URL the
// catalog carries for that listing — reference data, so the same whatever endpoint
// this emulator is served at and whatever host the reading request carried, and
// never an address of this emulator's own.
// ---------------------------------------------------------------------------

/// One addressing: where this emulator is SERVED, and which host the reading
/// request carried — neither of which a published listing depends on.
struct Addressing {
    case: &'static str,
    /// The base `OperationContext::endpoint` reports — the data port's own URL for
    /// a router served on a port, and the unserved base for one that is not.
    served_at: Option<&'static str>,
    /// The `Host` the reading request carried, or `None` for the request an SDK
    /// sends, which names this service's published endpoint in the URI.
    host: Option<&'static str>,
    /// An address THIS emulator answers that read on — the endpoint it is served
    /// at, or the host the reading request carried — which no published product
    /// URL may name.
    authority: &'static str,
}

/// The rows the address rule is read over: where this emulator is served (on no
/// port at all, on a loopback port, on an IPv6 port, at a name) crossed with the
/// host the reading request carried (a loopback address, a loopback name, one of
/// AWS's own hostnames, none at all).
fn addressings() -> Vec<Addressing> {
    vec![
        Addressing {
            case: "an SDK client naming this service's endpoint, on an unserved emulator",
            served_at: None,
            host: None,
            // `smithy_server_runtime::UNSERVED_URL_BASE`: the one base an emulator
            // served on no port reports.
            authority: "emulator.invalid",
        },
        Addressing {
            case: "a client on the loopback address of an unserved emulator",
            served_at: None,
            host: Some("127.0.0.1:9297"),
            authority: "127.0.0.1:9297",
        },
        Addressing {
            case: "an SDK client on an emulator served on a loopback port",
            served_at: Some("https://127.0.0.1:40305"),
            host: None,
            authority: "127.0.0.1:40305",
        },
        Addressing {
            case: "a client that resolves AWS's own hosts, on an emulator served on a port",
            served_at: Some("https://127.0.0.1:40305"),
            host: Some("eks.us-west-2.amazonaws.com"),
            authority: "127.0.0.1:40305",
        },
        Addressing {
            case: "a client naming the loopback by name, on an emulator served elsewhere",
            served_at: Some("https://127.0.0.1:40305"),
            host: Some("localhost:9297"),
            authority: "localhost:9297",
        },
        Addressing {
            case: "a client on an IPv6 literal, on an emulator served on an IPv6 port",
            served_at: Some("https://[::1]:9443"),
            host: Some("[::1]:9443"),
            authority: "[::1]:9443",
        },
        Addressing {
            case: "an SDK client on an emulator served at a name",
            served_at: Some("https://emulator.test:8443"),
            host: None,
            authority: "emulator.test:8443",
        },
    ]
}

/// The emulator under test, served at `base` — or on no port at all, which is what
/// [`emulator`] leaves it as.
fn emulator_served_at(base: Option<&str>) -> MultiServiceRouter {
    let mut router = emulator();
    if let Some(base) = base {
        router.set_url_base(
            EndpointUrl::parse(base).expect("the test endpoint base is a bare https URL"),
        );
    }
    router
}

/// The published catalog as read by a request that carried `host`, narrowed by
/// `query`.
async fn describe_addon_versions_from(
    router: &MultiServiceRouter,
    host: Option<&str>,
    query: &str,
) -> Answer {
    let mut builder = http::Request::builder()
        .method("GET")
        // What an SDK sends: this service's published endpoint in the URI.
        .uri(format!(
            "https://eks.{REGION}.amazonaws.com/addons/supported-versions?{query}"
        ))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={}/20260101/{REGION}/eks/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef",
                crate::wire::DEFAULT_ACCESS_KEY_ID
            ),
        );
    if let Some(host) = host {
        builder = builder.header("host", host);
    }
    let request = builder
        .body(bytes::Bytes::new())
        .expect("a well-formed catalog read");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body: serde_json::Value = serde_json::from_slice(response.body()).unwrap_or_default();
    assert_eq!(
        status, 200,
        "setup DescribeAddonVersions({query}) from {host:?}: expected HTTP 200, got {status} \
         {body:?}"
    );
    Answer {
        status,
        error_type: None,
        body,
    }
}

#[tokio::test]
async fn a_listings_product_url_is_the_marketplace_address_the_catalog_carries() {
    // One read per addressing, each walking every Marketplace add-on the catalog
    // publishes.
    let mut listings_per_addressing: Vec<(&str, BTreeMap<String, serde_json::Value>)> = Vec::new();
    for Addressing {
        case: addressing,
        served_at,
        host,
        authority,
    } in addressings()
    {
        let router = emulator_served_at(served_at);
        let answer = describe_addon_versions_from(
            &router,
            host,
            &format!("owners={ADDON_OWNER_MARKETPLACE}"),
        )
        .await;
        let published = addons(&answer);
        assert!(
            published.len() > 1,
            "case {addressing:?}: the catalog must publish several Marketplace add-ons, got {:?}",
            answer.body["addons"]
        );
        let mut listings: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        for addon in &published {
            let addon_name = text(addon, "addonName");
            let case = format!("{addressing:?} {addon_name}");
            let listing = &addon["marketplaceInformation"];
            if let Some(product_url) = listing.get("productUrl") {
                let product_url = product_url.as_str().unwrap_or_default();
                assert!(
                    marketplace_product_page(product_url).is_some(),
                    "case {case}: expected the listing's product page on the AWS Marketplace \
                     site, got {product_url:?}"
                );
                // Nothing of the address this read was answered on, and nothing of
                // the path namespace this emulator serves its own planes under: a
                // product URL identifies a product, not an endpoint.
                for local in [authority, "_aws", "emulator.invalid"] {
                    assert!(
                        !product_url.contains(local),
                        "case {case}: a product URL must not be composed from the address this \
                         emulator is reached on ({local}), got {product_url:?}"
                    );
                }
            }
            listings.insert(addon_name, listing.clone());
        }
        listings_per_addressing.push((addressing, listings));
    }

    // The listing does not move with the addressing at all: it is the Marketplace
    // catalog's data, not this emulator's.
    let (first_case, first) = &listings_per_addressing[0];
    for (case, listings) in &listings_per_addressing {
        assert_eq!(
            listings, first,
            "case {case:?}: the listings must be the ones {first_case:?} read too, got {listings:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_with_no_published_product_page_reports_no_product_url() {
    let router = emulator_served_at(Some("https://127.0.0.1:40305"));
    let answer =
        describe_addon_versions_from(&router, None, &format!("owners={ADDON_OWNER_MARKETPLACE}"))
            .await;

    // Which add-ons publish a product page and which do not is the catalog's
    // business; what the rule fixes is that the member is either that page's
    // address or ABSENT — never null, never empty, and never an address of this
    // emulator's.
    let mut with_page = 0_usize;
    let mut without_page = 0_usize;
    for addon in &addons(&answer) {
        let case = text(addon, "addonName");
        let listing = &addon["marketplaceInformation"];
        assert!(
            !text(listing, "productId").is_empty(),
            "case {case}: a listing reports its Marketplace product id, got {listing:?}"
        );
        match listing.get("productUrl") {
            None => without_page += 1,
            Some(product_url) => {
                let product_url = product_url.as_str().unwrap_or_else(|| {
                    panic!("case {case}: a published product URL is text, got {product_url:?}")
                });
                assert!(
                    marketplace_product_page(product_url).is_some(),
                    "case {case}: expected the listing's product page on the AWS Marketplace \
                     site, got {product_url:?}"
                );
                with_page += 1;
            }
        }
    }
    assert!(
        with_page > 0,
        "the catalog must publish at least one listing's product page, got {with_page} with and \
         {without_page} without"
    );
}

#[tokio::test]
async fn each_listing_reports_the_product_page_the_catalog_carries_for_it() {
    let router = emulator();

    // The rows name, per add-on, the page the catalog carries for its listing, each
    // in the form AWS Marketplace publishes that product page under — and, for a
    // listing whose published page the catalog cannot CITE, no page at all: the
    // member is then absent rather than carrying an address nobody published. They
    // span both published forms, three sellers and a vendor's rebuild of an Amazon
    // EKS-managed add-on.
    let cases: Vec<(&str, Option<&str>)> = vec![
        (
            "accuknox_kubearmor",
            Some("https://aws.amazon.com/marketplace/pp/prodview-5npox4ixz64qk"),
        ),
        (
            "netapp_trident-operator",
            Some("https://aws.amazon.com/marketplace/pp?sku=3anbrahbid63isddiigf6j5qy"),
        ),
        ("chainguard_aws-efs-csi-driver-fips", None),
        ("datadog_operator", None),
    ];

    for (case, expected) in cases {
        let answer = describe_addon_versions(&router, &format!("addonName={case}")).await;
        let published = addons(&answer);
        assert_eq!(
            published.len(),
            1,
            "case {case}: a name the catalog publishes answers exactly that add-on, got {:?}",
            answer.body["addons"]
        );
        let listing = &published[0]["marketplaceInformation"];
        let product_url = listing
            .get("productUrl")
            .and_then(serde_json::Value::as_str);
        assert_eq!(
            product_url, expected,
            "case {case}: expected the product page the catalog carries, {expected:?}, got {:?}",
            listing["productUrl"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a product page is published at the AWS Marketplace product page address
// followed by the listing's own key, in one of the two forms AWS Marketplace
// publishes — and a listing the catalog cannot cite a page for publishes none.
// ---------------------------------------------------------------------------

#[test]
fn a_published_product_page_is_addressed_in_one_of_the_marketplace_forms() {
    // The rows span the two published forms and the listing that carries neither.
    let cases: Vec<(&str, MarketplaceProductPage, Option<&str>)> = vec![
        (
            "the product-view form",
            MarketplaceProductPage::ProductView("prodview-5npox4ixz64qk"),
            Some("https://aws.amazon.com/marketplace/pp/prodview-5npox4ixz64qk"),
        ),
        (
            "the stock-keeping-unit form",
            MarketplaceProductPage::StockKeepingUnit("3anbrahbid63isddiigf6j5qy"),
            Some("https://aws.amazon.com/marketplace/pp?sku=3anbrahbid63isddiigf6j5qy"),
        ),
        (
            "a listing whose page the catalog cannot cite",
            MarketplaceProductPage::Uncited,
            None,
        ),
    ];

    for (case, page, expected) in cases {
        let product_url = page.product_url();
        assert_eq!(
            product_url.as_deref(),
            expected,
            "case {case}: expected {expected:?}, got {product_url:?}"
        );
        if let Some(product_url) = &product_url {
            assert!(
                marketplace_product_page(product_url).is_some(),
                "case {case}: expected an AWS Marketplace product page, got {product_url:?}"
            );
        }
    }
}
