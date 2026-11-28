// SPDX-License-Identifier: CC0-1.0

//! What a drill-down detail view reports about the record the requested id names.
//!
//! The rules under test:
//!
//! * A product's fulfillment is ONE record. `Product` carries a `FulfillmentOptionsList`
//!   and `FulfillmentOption` is a union documented "Each element contains exactly one
//!   fulfillment option type", so every view of that product restates those options and
//!   only those: `fulfillmentOptionSummaries` (on `GetProduct`, on `GetListing` and in a
//!   `SearchListings` summary) reports one summary per distinct type the record offers, and
//!   `ListFulfillmentOptions` serves each option as the union member of its OWN type. A
//!   type the record does not offer appears in no view — so no member that belongs to
//!   another arm (an AMI's `operatingSystems`, `architecture` or `accessUrlTemplate`, say)
//!   is served for a product that is not fulfilled that way.
//! * An optional attribute is claimed only where the record carries it:
//!   `SaasFulfillmentOption$quickLaunch` ("Specifies whether the SaaS product supports
//!   quick-launch deployment") is `ENABLED` only for a record carrying the `launchUrl`
//!   ("The URL that a buyer uses to launch the seller's SaaS product") such a launch sends
//!   the buyer to, and a listing's `QUICK_LAUNCH` badge appears exactly where one of its
//!   products has such an option.
//! * An address a fulfillment option PUBLISHES for a buyer to follow (a SaaS option's
//!   `fulfillmentUrl` registration landing page and the `launchUrl` a launch uses) is served
//!   by THIS emulator, on the address this process answers requests at, and answers
//!   there with the stored option — and an address of that shape naming a page no record
//!   names is served as no page at all.
//! * An optional member of an option is restated off the record: a SaaS option's
//!   `usageInstructions` ("Instructions on how to access and use this SaaS product") is the
//!   seller's own text, so a record that states none carries none rather than a sentence
//!   composed from the product's name.
//! * `Category$categoryId` is "The machine-readable identifier of the category" and
//!   `displayName` "The human-readable name of the category", so the identifier is the
//!   catalog's own opaque handle rather than a form of the name it renders: a client can
//!   persist it, read it back unchanged from every view, and pass it back as a `CATEGORY`
//!   filter value to select exactly the listings whose records carry it.
//!
//! How many options a product offers, and the paging over them, is asserted in
//! `fulfillment_option_families`; the deployment status a product's options imply, and the
//! order its categories come back in, in `catalog_record_breadth`.
//!
//! Every request goes through the `MultiServiceRouter`, so the protocol, the routing table
//! and the handlers are all exercised end to end.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::state::{
    category_id, saas_page_url, SaasPage,
};

use crate::catalog_drilldown::{array, ok, router, text, whole_catalog};

/// Every value of the model's `FulfillmentOptionType` enum and the `FulfillmentOption`
/// union member that carries an option of that type.
///
/// Source: `smithy_query(query="shape",shape="FulfillmentOptionType")` for the values and
/// `smithy_query(query="shape",shape="FulfillmentOption")` for the members.
const UNION_MEMBER_OF_TYPE: &[(&str, &str)] = &[
    ("AMAZON_MACHINE_IMAGE", "amazonMachineImageFulfillmentOption"),
    ("API", "apiFulfillmentOption"),
    ("CLOUDFORMATION_TEMPLATE", "cloudFormationFulfillmentOption"),
    ("CONTAINER", "containerFulfillmentOption"),
    ("HELM", "helmFulfillmentOption"),
    ("EKS_ADD_ON", "eksAddOnFulfillmentOption"),
    (
        "EC2_IMAGE_BUILDER_COMPONENT",
        "ec2ImageBuilderComponentFulfillmentOption",
    ),
    ("DATA_EXCHANGE", "dataExchangeFulfillmentOption"),
    (
        "PROFESSIONAL_SERVICES",
        "professionalServicesFulfillmentOption",
    ),
    ("SAAS", "saasFulfillmentOption"),
    ("SAGEMAKER_ALGORITHM", "sageMakerAlgorithmFulfillmentOption"),
    ("SAGEMAKER_MODEL", "sageMakerModelFulfillmentOption"),
];

/// Members only an `AmazonMachineImageFulfillmentOption` declares, which therefore may not
/// be served for a product that offers no AMI fulfillment. (`operatingSystems`,
/// `releaseNotes` and `usageInstructions` are NOT here: other arms declare them too.)
///
/// Source: `smithy_query(query="shape",shape="AmazonMachineImageFulfillmentOption")` read
/// against every other member of the `FulfillmentOption` union.
const AMI_ONLY_MEMBERS: &[&str] = &[
    "architecture",
    "accessUrlTemplate",
    "amiAlias",
    "ebsVolume",
];

/// Categories of the catalog's taxonomy the identifier rule is checked over: one at each of
/// the taxonomy's three levels, under three different top-level categories, and entries the
/// issue that prompted this suite never cited.
///
/// Source: the taxonomy the catalog declares (`state::CATEGORY_TAXONOMY`, read off the AWS
/// Marketplace browse taxonomy), as `(taxonomy key, displayName)`.
const CATEGORY_ROWS: &[(&str, &str)] = &[
    ("infrastructure-software", "Infrastructure Software"),
    ("storage", "Storage"),
    ("data-analytics", "Data Analytics"),
    ("data-integration", "Data Integration"),
    ("iot-edge-computing", "IoT Edge Computing"),
    ("structured-data-ml", "Structured Data"),
    ("training", "Training"),
];

/// Whether `value` has the form of an identifier this catalog issues for a category: the
/// 36-character lower-case hyphenated form, which carries no display text.
fn is_issued_identifier(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok() && value.len() == 36 && value == value.to_lowercase()
}

/// The single union member of a `FulfillmentOption`: its name and its value.
fn union_member(case: &str, option: &Value) -> (String, Value) {
    let object = option
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: a fulfillment option must be an object, got {option}"));
    assert_eq!(
        object.len(),
        1,
        "case {case}: a FulfillmentOption carries exactly one member, got {option}"
    );
    let (name, value) = object.iter().next().expect("the one member");
    (name.clone(), value.clone())
}

/// The `fulfillmentOptionType` of every summary of a record.
fn summarised_types(record: &Value) -> Vec<String> {
    array(record, "fulfillmentOptionSummaries")
        .iter()
        .map(|summary| text(summary, "fulfillmentOptionType"))
        .collect()
}

/// Every fulfillment option of `product_id`, one whole page.
async fn options_of(router: &MultiServiceRouter, product_id: &str) -> Vec<Value> {
    let body = ok(
        router,
        "listFulfillmentOptions",
        &json!({ "productId": product_id, "maxResults": 50 }),
    )
    .await;
    assert!(
        body.get("nextToken").is_none_or(Value::is_null),
        "case {product_id}: no product offers more than one 50-item page, got {body}"
    );
    array(&body, "fulfillmentOptions")
}

/// RULE: every view of a product restates the fulfillment options its record carries, and
/// only those — over every product in the catalog, which differ in option count and type.
#[tokio::test]
async fn every_view_of_a_product_restates_the_options_its_record_carries() {
    let router = router();

    let mut products_seen = 0_usize;
    let mut arms_seen: BTreeSet<String> = BTreeSet::new();
    for summary in whole_catalog(&router).await {
        let listing_id = text(&summary, "listingId");
        let listing = ok(
            &router,
            "getListing",
            &json!({ "listingId": listing_id.clone() }),
        )
        .await;
        // The listing is fulfilled through its products, so its summaries are the
        // de-duplicated union of theirs and name no further type.
        let mut listing_types: Vec<String> = Vec::new();

        for entity in array(&listing, "associatedEntities") {
            let Some(product) = entity.get("product") else {
                continue;
            };
            let product_id = text(product, "productId");
            let detail = ok(
                &router,
                "getProduct",
                &json!({ "productId": product_id.clone() }),
            )
            .await;
            let options = options_of(&router, &product_id).await;
            assert!(
                !options.is_empty(),
                "case {product_id}: a catalog product must report how it is fulfilled"
            );

            // Each option is the union member its own type declares.
            let mut served: Vec<String> = Vec::new();
            for option in &options {
                let (member, body) = union_member(&product_id, option);
                let option_type = text(&body, "fulfillmentOptionType");
                let expected = UNION_MEMBER_OF_TYPE
                    .iter()
                    .find(|(value, _)| *value == option_type)
                    .map(|(_, member)| *member);
                assert_eq!(
                    expected,
                    Some(member.as_str()),
                    "case {product_id}: an option of type {option_type} must be served as its \
                     own union member, got {member}"
                );
                assert!(
                    !text(&body, "fulfillmentOptionDisplayName").trim().is_empty(),
                    "case {product_id}: fulfillmentOptionDisplayName is required and non-blank, \
                     got {body}"
                );
                arms_seen.insert(member);
                if !served.contains(&option_type) {
                    served.push(option_type);
                }
            }

            // The product summarises exactly the types it serves, in the same order.
            assert_eq!(
                summarised_types(&detail),
                served,
                "case getProduct {product_id}: the summaries must restate the types the record \
                 offers, got {:?}",
                summarised_types(&detail)
            );
            for summary in array(&detail, "fulfillmentOptionSummaries") {
                assert!(
                    !text(&summary, "displayName").trim().is_empty(),
                    "case getProduct {product_id}: FulfillmentOptionSummary$displayName is \
                     required and non-blank, got {summary}"
                );
            }

            // A member of another arm is served for no product: an AMI-only member appears
            // only where the record offers AMI fulfillment.
            let offers_ami = served.iter().any(|value| value == "AMAZON_MACHINE_IMAGE");
            for option in &options {
                let (_, body) = union_member(&product_id, option);
                for member in AMI_ONLY_MEMBERS {
                    assert!(
                        offers_ami || body.get(*member).is_none(),
                        "case {product_id} fulfilled as {served:?}: {member} belongs to the AMI \
                         option alone, got {body}"
                    );
                }
            }

            for option_type in served {
                if !listing_types.contains(&option_type) {
                    listing_types.push(option_type);
                }
            }
            products_seen += 1;
        }

        assert_eq!(
            summarised_types(&listing),
            listing_types,
            "case getListing {listing_id}: the listing must summarise the types its products \
             offer, got {:?}",
            summarised_types(&listing)
        );
        assert_eq!(
            summarised_types(&summary),
            listing_types,
            "case searchListings {listing_id}: a summary must report the same types the detail \
             read does, got {:?}",
            summarised_types(&summary)
        );
    }

    assert!(
        products_seen > 10,
        "case the whole catalog: the scan must reach the catalog's products, got \
         {products_seen}"
    );
    assert_eq!(
        arms_seen.len(),
        UNION_MEMBER_OF_TYPE.len(),
        "case the whole catalog: every modelled fulfillment arm must stay reachable, got \
         {arms_seen:?}"
    );
}

/// RULE: quick launch is claimed only where the record carries the launch URL such a launch
/// uses, and the listing badge that restates it appears exactly there.
#[tokio::test]
async fn quick_launch_is_claimed_only_where_the_record_carries_a_launch_url() {
    let router = router();

    let mut enabled = 0_usize;
    let mut disabled = 0_usize;
    for summary in whole_catalog(&router).await {
        let listing_id = text(&summary, "listingId");
        let listing = ok(
            &router,
            "getListing",
            &json!({ "listingId": listing_id.clone() }),
        )
        .await;
        let mut quick_launchable = false;
        for entity in array(&listing, "associatedEntities") {
            let Some(product) = entity.get("product") else {
                continue;
            };
            let product_id = text(product, "productId");
            for option in options_of(&router, &product_id).await {
                let (member, body) = union_member(&product_id, &option);
                if member != "saasFulfillmentOption" {
                    continue;
                }
                let quick_launch = text(&body, "quickLaunch");
                // Source: `smithy_query(query="shape",shape="SaasQuickLaunchStatus")`.
                assert!(
                    ["ENABLED", "DISABLED"].contains(&quick_launch.as_str()),
                    "case {product_id}: quickLaunch must be a modelled status, got \
                     {quick_launch:?}"
                );
                if quick_launch == "ENABLED" {
                    assert!(
                        body.get("launchUrl").is_some(),
                        "case {product_id}: a quick-launchable option must carry the launchUrl \
                         such a launch uses, got {body}"
                    );
                    quick_launchable = true;
                    enabled += 1;
                } else {
                    disabled += 1;
                }
            }
        }
        let badges: BTreeSet<String> = array(&listing, "badges")
            .iter()
            .map(|badge| text(badge, "badgeType"))
            .collect();
        assert_eq!(
            badges.contains("QUICK_LAUNCH"),
            quick_launchable,
            "case getListing {listing_id}: the QUICK_LAUNCH badge must restate whether a \
             product of the listing supports it, got {badges:?}"
        );
    }

    assert!(
        enabled > 0 && disabled > 0,
        "case the whole catalog: both quick-launch statuses must stay reachable, got {enabled} \
         enabled and {disabled} disabled"
    );
}

/// RULE: a category is named on the wire by the identifier the catalog issues for it — an
/// opaque identifier rather than a form of its display name — and that identifier is what a
/// `CATEGORY` filter selects by.
#[tokio::test]
async fn a_category_is_named_by_an_opaque_identifier_that_round_trips_as_a_filter() {
    let router = router();

    // Which listings carry which category, read off the detail view of every listing.
    let mut carriers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    for summary in whole_catalog(&router).await {
        let listing_id = text(&summary, "listingId");
        let listing = ok(
            &router,
            "getListing",
            &json!({ "listingId": listing_id.clone() }),
        )
        .await;
        for category in array(&listing, "categories") {
            let id = text(&category, "categoryId");
            let display_name = text(&category, "displayName");
            assert!(
                is_issued_identifier(&id),
                "case getListing {listing_id}: categoryId must be an identifier this service \
                 issues, got {id:?} for {display_name:?}"
            );
            assert_ne!(
                id, display_name,
                "case getListing {listing_id}: the identifier and the display name are \
                 different members"
            );
            names.insert(id.clone(), display_name);
            carriers.entry(id).or_default().insert(listing_id.clone());
        }
    }

    for (key, display_name) in CATEGORY_ROWS {
        let id = category_id(key);
        assert!(
            is_issued_identifier(&id),
            "case {key}: the issued identifier must have the form this service issues, got {id:?}"
        );
        // It carries none of the display name: not the name itself, not a case-folded or
        // hyphenated form of it, and none of its words.
        let folded = display_name.to_lowercase();
        assert_ne!(id, folded.replace(' ', "-"), "case {key}: the identifier must not be a slug of {display_name:?}");
        for word in folded.split(' ').filter(|word| word.len() > 2) {
            assert!(
                !id.contains(word),
                "case {key}: the identifier must carry none of the display name, got {id:?} \
                 containing {word:?}"
            );
        }
        assert_eq!(
            names.get(&id).map(String::as_str),
            Some(*display_name),
            "case {key}: the catalog must report {display_name:?} for {id}, got {:?}",
            names.get(&id)
        );

        // Passing it back as a `CATEGORY` filter value selects exactly the listings whose
        // records carry it.
        let body = ok(
            &router,
            "searchListings",
            &json!({
                "maxResults": 100,
                "filters": [{ "filterType": "CATEGORY", "filterValues": [id.clone()] }],
            }),
        )
        .await;
        let selected: BTreeSet<String> = array(&body, "listingSummaries")
            .iter()
            .map(|summary| text(summary, "listingId"))
            .collect();
        let expected = carriers.get(&id).cloned().unwrap_or_default();
        assert!(
            !expected.is_empty(),
            "case {key}: the catalog must hold a listing classified under it"
        );
        assert_eq!(
            selected, expected,
            "case CATEGORY={id} ({key}): the filter must select the listings whose records \
             carry it, got {selected:?}"
        );

        // And a `CATEGORY` facet reports the same identifier, so the value a client refines
        // by is the one the records carry.
        let facets = ok(
            &router,
            "searchFacets",
            &json!({ "facetTypes": ["CATEGORY"] }),
        )
        .await;
        let reported: BTreeSet<String> = facets
            .get("listingFacets")
            .and_then(Value::as_object)
            .and_then(|map| map.get("CATEGORY"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|facet| text(facet, "value"))
            .collect();
        assert!(
            reported.contains(&id),
            "case {key}: the CATEGORY facet must report {id}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The addresses a SaaS fulfillment option publishes for a buyer to FOLLOW.
// ---------------------------------------------------------------------------

/// The endpoint bases this emulator can report as its own: the forms a data port is
/// published at, plus the base of an emulator served on no port at all (embedded in another
/// process, or a test). One row each, because an address an option publishes has to follow
/// whichever one this process is served at.
const ENDPOINT_BASES: [&str; 4] = [
    "https://127.0.0.1:9297",
    "https://localhost:39355",
    "https://[::1]:8443",
    smithy_server_runtime::UNSERVED_URL_BASE,
];

/// Hosts this emulator does not answer on: the zones AWS publishes its own endpoints in, and
/// the domain reserved for documentation examples. A published address may name none of them,
/// because nothing serves a fetch of one.
const UNSERVED_HOSTS: [&str; 4] = ["example.com", ".amazonaws.com", ".api.aws", ".on.aws"];

/// The optional URL members of a SaaS option, with the page each one publishes the address
/// of.
///
/// Source: `smithy_query(query="shape",shape="SaasFulfillmentOption")` — `fulfillmentUrl`
/// ("The URL of the seller's software registration landing page") and `launchUrl` ("The URL
/// that a buyer uses to launch the seller's SaaS product"), both optional.
const PUBLISHED_PAGES: [(&str, SaasPage); 2] = [
    ("fulfillmentUrl", SaasPage::Registration),
    ("launchUrl", SaasPage::Launch),
];

/// The path prefix every address this service publishes for a fulfillment option carries:
/// the reserved namespace segment of a plane served at the emulator's own front door,
/// followed by the segment naming this service's SaaS fulfillment plane.
const PLANE_PREFIX: &str = "/_aws/marketplace-discovery-saas-fulfillment";

/// A router whose `OperationContext::endpoint` is `base`, which is the address the pages a
/// SaaS option publishes have to be announced on.
fn router_served_at(base: &str) -> MultiServiceRouter {
    let mut router = router();
    router.set_url_base(
        smithy_server_runtime::EndpointUrl::parse(base).expect("a base URL the router accepts"),
    );
    router
}

/// One page a client FETCHES: the status, the content type and the document.
struct Page {
    status: u16,
    content_type: String,
    body: String,
}

/// Fetch `url` the way a browser holding a published address does: no credential and no
/// target header, just the address the option announced.
async fn fetch(router: &MultiServiceRouter, method: &str, url: &str) -> Page {
    let (host, path) = split_url(url);
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .body(bytes::Bytes::new())
        .expect("build the request");
    let response = router.dispatch(request).await;
    Page {
        status: response.status().as_u16(),
        content_type: response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string(),
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
}

/// The authority and the path of an absolute URL, which is how a client addresses it.
fn split_url(url: &str) -> (String, String) {
    let without_scheme = url.split_once("://").map_or(url, |(_scheme, rest)| rest);
    match without_scheme.find('/') {
        Some(at) => (
            without_scheme[..at].to_string(),
            without_scheme[at..].to_string(),
        ),
        None => (without_scheme.to_string(), "/".to_string()),
    }
}

/// Every fulfillment option in the catalog, as `(productId, productName, option body)` with
/// the union member it was served as.
async fn catalog_options(router: &MultiServiceRouter) -> Vec<(String, String, String, Value)> {
    let mut options = Vec::new();
    for summary in whole_catalog(router).await {
        for entity in array(&summary, "associatedEntities") {
            let Some(product) = entity.get("product") else {
                continue;
            };
            let product_id = text(product, "productId");
            let product_name = text(product, "productName");
            for option in options_of(router, &product_id).await {
                let (member, body) = union_member(&product_id, &option);
                options.push((product_id.clone(), product_name.clone(), member, body));
            }
        }
    }
    assert!(
        !options.is_empty(),
        "case setup: the seeded catalog must hold fulfillment options"
    );
    options
}

/// Every SaaS fulfillment option in the catalog, as `(productId, productName, option body)`.
async fn saas_options(router: &MultiServiceRouter) -> Vec<(String, String, Value)> {
    catalog_options(router)
        .await
        .into_iter()
        .filter(|(_, _, member, _)| member == "saasFulfillmentOption")
        .map(|(product_id, product_name, _, body)| (product_id, product_name, body))
        .collect()
}

/// RULE: each buyer-facing page a SaaS option's record names is announced at the endpoint
/// THIS emulator is served at, with the option's own id in the path, and a fetch of that
/// address is answered here from the stored option; the address of a page the record does not
/// name is announced by no member and is served as no page.
#[tokio::test]
async fn a_saas_option_publishes_each_page_its_record_names_on_this_emulators_endpoint() {
    for base in ENDPOINT_BASES {
        let router = router_served_at(base);
        let options = saas_options(&router).await;
        assert!(
            !options.is_empty(),
            "case {base:?}: the catalog must hold SaaS fulfillment options"
        );

        let mut announced = 0_usize;
        let mut withheld = 0_usize;
        for (product_id, _, option) in &options {
            let option_id = text(option, "fulfillmentOptionId");
            let quick_launch = text(option, "quickLaunch");
            for (member, page_kind) in PUBLISHED_PAGES {
                // The address this service publishes for that page, composed through the
                // service's own ONE statement of the layout — the address carries the
                // authenticator this process minted for the option
                // (`state::media_address`), which only that statement can produce.
                let address = saas_page_url(base, &option_id, page_kind);
                let Some(published) = option.get(member).and_then(Value::as_str) else {
                    // The record names no such page, so no member announces one — and the
                    // address it would have is served as nothing.
                    let page = fetch(&router, "GET", &address).await;
                    assert_eq!(
                        page.status, 404,
                        "case {base:?} {option_id} {member}: an address no record names must \
                         be served as no page, got {} ({:?})",
                        page.status, page.body
                    );
                    assert!(
                        !page.body.contains(&option_id),
                        "case {base:?} {option_id} {member}: a page nothing stands behind must \
                         restate no record, got {:?}",
                        page.body
                    );
                    withheld += 1;
                    continue;
                };
                assert_eq!(
                    published, address,
                    "case {base:?} {option_id} {member}: expected the page published on this \
                     emulator's own endpoint, got {published:?}"
                );
                for host in UNSERVED_HOSTS {
                    assert!(
                        !published.contains(host),
                        "case {base:?} {option_id} {member}: a published address must name no \
                         host this emulator does not serve, got {published:?}"
                    );
                }

                let page = fetch(&router, "GET", published).await;
                assert_eq!(
                    page.status, 200,
                    "case {base:?} {option_id} {member}: expected {published:?} to be served \
                     here, got {} ({:?})",
                    page.status, page.body
                );
                assert!(
                    page.content_type.starts_with("text/html"),
                    "case {base:?} {option_id} {member}: expected a buyer-facing page, got \
                     content type {:?}",
                    page.content_type
                );
                for stated in [option_id.as_str(), product_id.as_str(), quick_launch.as_str()] {
                    assert!(
                        page.body.contains(stated),
                        "case {base:?} {option_id} {member}: expected the page to restate \
                         {stated:?} off the record, got {:?}",
                        page.body
                    );
                }

                // What a client asks when it only wants to know the page is there.
                let head = fetch(&router, "HEAD", published).await;
                assert_eq!(
                    (head.status, head.body.is_empty()),
                    (200, true),
                    "case {base:?} {option_id} {member}: expected a HEAD of {published:?} to \
                     answer 200 with no body, got {} ({:?})",
                    head.status,
                    head.body
                );
                announced += 1;
            }
        }

        assert!(
            announced > 0 && withheld > 0,
            "case {base:?}: the catalog must hold records that name a page and records that do \
             not, so the presence of each member carries information, got {announced} announced \
             and {withheld} withheld"
        );
    }
}

/// RULE: an optional text member of a fulfillment option is restated off the stored record,
/// so it is emitted only where the record holds a value for it and is never composed at
/// projection time from the product's own name.
///
/// Checked over every SaaS option in the catalog, which is where the catalog holds records
/// both with and without the member: `usageInstructions` ("Instructions on how to access and
/// use this SaaS product") is the seller's own text, so a record that states none carries
/// none and a client branching on the member sees that.
#[tokio::test]
async fn a_fulfillment_option_carries_optional_text_only_where_its_record_states_it() {
    let router = router();

    let mut stated = 0_usize;
    let mut absent = 0_usize;
    for (product_id, product_name, option) in saas_options(&router).await {
        let option_id = text(&option, "fulfillmentOptionId");
        match option.get("usageInstructions") {
            Some(value) => {
                let instructions = value.as_str().unwrap_or_default();
                assert!(
                    !instructions.trim().is_empty(),
                    "case {product_id} {option_id}: a member the record states must carry its \
                     value, got {value}"
                );
                assert!(
                    !instructions.contains(&product_name),
                    "case {product_id} {option_id}: usageInstructions is the seller's own text, \
                     not a sentence composed from the product name {product_name:?}, got \
                     {instructions:?}"
                );
                stated += 1;
            }
            None => absent += 1,
        }
    }

    assert!(
        stated > 0 && absent > 0,
        "case the whole catalog: both a record that states usage instructions and one that \
         states none must stay reachable, so the presence of the member carries information, \
         got {stated} stating and {absent} not"
    );
}

/// RULE: only an address this service PUBLISHED is served as a buyer-facing page: an address
/// of the same shape naming an option the catalog does not hold, an option of another
/// fulfillment type, or no page at all is served as no page.
#[tokio::test]
async fn only_a_published_address_is_served_as_a_buyer_facing_page() {
    let router = router();
    let base = smithy_server_runtime::UNSERVED_URL_BASE;

    let options = catalog_options(&router).await;
    let saas_id = options
        .iter()
        .find(|(_, _, member, _)| member == "saasFulfillmentOption")
        .map(|(_, _, _, option)| text(option, "fulfillmentOptionId"))
        .expect("the catalog holds a SaaS fulfillment option");
    let other_id = options
        .iter()
        .find(|(_, _, member, _)| member != "saasFulfillmentOption")
        .map(|(_, _, _, option)| text(option, "fulfillmentOptionId"))
        .expect("the catalog holds an option of another fulfillment type");

    // One row per way an address can fail to name a page this service published.
    let cases: Vec<(String, String)> = vec![
        (
            "an option this catalog does not hold".to_string(),
            format!("{base}{PLANE_PREFIX}/fo-0000-no-such-option-saas/register"),
        ),
        (
            "an option of another fulfillment type".to_string(),
            format!("{base}{PLANE_PREFIX}/{other_id}/register"),
        ),
        (
            "a page this service publishes for no option".to_string(),
            format!("{base}{PLANE_PREFIX}/{saas_id}/manage"),
        ),
        (
            "a path below a page, which is one document rather than a path space".to_string(),
            format!("{base}{PLANE_PREFIX}/{saas_id}/register/terms"),
        ),
        (
            "no option named at all".to_string(),
            format!("{base}{PLANE_PREFIX}/register"),
        ),
    ];

    for (case, address) in cases {
        let page = fetch(&router, "GET", &address).await;
        assert_ne!(
            page.status, 200,
            "case {case}: {address:?} names no page this service published, got {} ({:?})",
            page.status, page.body
        );
        assert!(
            !page.body.contains(&saas_id) && !page.body.contains(&other_id),
            "case {case}: an answer for {address:?} must restate no record, got {:?}",
            page.body
        );
    }

    // And the address this service DID publish is still served, so the refusals above are
    // the shape of the address rather than the route being unreachable.
    let option = options
        .iter()
        .find(|(_, _, _, option)| text(option, "fulfillmentOptionId") == saas_id)
        .map(|(_, _, _, option)| option.clone())
        .expect("the SaaS option just read");
    let published = option
        .get("fulfillmentUrl")
        .and_then(Value::as_str)
        .expect("a SaaS record naming a registration page publishes it")
        .to_string();
    let page = fetch(&router, "GET", &published).await;
    assert_eq!(
        page.status, 200,
        "case the published address {published:?}: expected it to be served here, got {} ({:?})",
        page.status, page.body
    );
}

/// The addresses a client can have DIALLED to reach this emulator, with whether that address
/// can lead a later fetch BACK to this process — which is what makes it the one a published
/// page address names.
///
/// One row each: the authority a client that dialled this emulator stated (its own spelling of
/// it, and a port this process is not served on but was reached through, as a published
/// container port or a forwarding front end reaches it), and otherwise the address this process
/// is served at — for an authority naming no port where this process is not served on the one
/// `https` implies, and for a host AWS publishes its own service endpoints at.
const DIALLED_HOSTS: [(&str, bool); 6] = [
    ("127.0.0.1:9297", true),
    ("localhost:9297", true),
    ("localhost:39355", true),
    ("[::1]:8443", true),
    ("127.0.0.1", false),
    ("discovery-marketplace.us-east-1.amazonaws.com", false),
];

/// Call `listFulfillmentOptions` as a client that dialled `host`: the address it resolved and
/// reached this emulator at, carried both in the `Host` header and in the request URI.
async fn options_dialled_at(
    router: &MultiServiceRouter,
    host: &str,
    product_id: &str,
) -> Vec<Value> {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/2026-02-05/listFulfillmentOptions"))
        .header("host", host)
        .header("authorization", crate::catalog_drilldown::AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(
            json!({ "productId": product_id, "maxResults": 50 }).to_string(),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case dialled {host}: expected HTTP 200, got {} ({:?})",
        response.status().as_u16(),
        String::from_utf8_lossy(response.body())
    );
    let body: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    array(&body, "fulfillmentOptions")
}

/// RULE: the address a published page is announced on is the authority the answering request
/// arrived on — as the client that dialled it spelled it — and the address this process is
/// served at for a request carrying no authority that can lead back here, so a client holding
/// the URL can dial it again.
#[tokio::test]
async fn a_published_address_names_the_authority_the_request_arrived_on() {
    // The emulator is served on a port of its own, which is the address a caller that reached
    // it any other way is told about.
    let served = "https://127.0.0.1:9297";
    let router = router_served_at(served);
    let product_id = saas_options(&router)
        .await
        .first()
        .map(|(product_id, _, _)| product_id.clone())
        .expect("the catalog holds a product fulfilled as SaaS");

    for (host, leads_back_here) in DIALLED_HOSTS {
        let expected_base = if leads_back_here {
            format!("https://{host}")
        } else {
            served.to_string()
        };
        let options = options_dialled_at(&router, host, &product_id).await;
        let mut seen = 0_usize;
        for option in options {
            let (member, body) = union_member(&product_id, &option);
            if member != "saasFulfillmentOption" {
                continue;
            }
            let option_id = text(&body, "fulfillmentOptionId");
            let published = body
                .get("fulfillmentUrl")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                published,
                saas_page_url(&expected_base, &option_id, SaasPage::Registration),
                "case dialled {host}: expected the page announced on the authority this request \
                 arrived on, got {published:?}"
            );
            let page = fetch(&router, "GET", published).await;
            assert_eq!(
                page.status, 200,
                "case dialled {host}: expected {published:?} to be served here, got {} ({:?})",
                page.status, page.body
            );
            seen += 1;
        }
        assert!(
            seen > 0,
            "case dialled {host}: the product read must report its SaaS option"
        );
    }
}
