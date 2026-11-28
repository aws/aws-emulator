// SPDX-License-Identifier: CC0-1.0

//! What a drill-down detail view answers from, and what a SaaS fulfillment option names.
//!
//! The rules under test:
//!
//! * A content member of a detail view is a property OF THE RECORD the requested id
//!   resolves to — `highlights` is "[a] list of key features that the listing offers to
//!   customers" (`smithy_query(query="operation",operation="GetListing")`) — so every member
//!   `GetListing` and `GetProduct` report is read off that record, and two different records
//!   state different key features rather than one list every record delivered the same way
//!   restates.
//! * A SaaS fulfillment option names the deployment templates it offers: `ListFulfillmentOptions`
//!   carries a `deploymentTemplates` member inside every `saasFulfillmentOption`, present and
//!   holding no element for an option the catalog holds no template for, and an absent member
//!   is a different case to a typed client than a member holding an empty list. The member
//!   belongs to the SaaS arm of the `FulfillmentOption` union, so no option of another type
//!   carries it.
//!
//! Every suite drives real HTTP requests through the `MultiServiceRouter`, so the protocol,
//! the routing table and the handlers are all exercised end to end.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::catalog_drilldown::{array, ok, router, text};

/// The member a SaaS option names its deployment templates in.
///
/// `SaasFulfillmentOption$deploymentTemplates` is `@required` with a default of `[]`
/// (`smithy_query(query="shape",shape="SaasFulfillmentOption")`), and this catalog publishes
/// no deployment template for any of its SaaS options, so the member reaches the wire as a
/// present, empty list.
const DEPLOYMENT_TEMPLATES: &str = "deploymentTemplates";

/// The union member a SaaS option is served as
/// (`smithy_query(query="shape",shape="FulfillmentOption")`).
const SAAS_MEMBER: &str = "saasFulfillmentOption";

/// How many listings the per-record round trips below are run over: the suites that read a
/// record back through every view take a sample spanning the catalog's tiers, which keeps the
/// call count bounded while still covering hand-written records (first in listing-id order)
/// and derived ones.
const SAMPLED_LISTINGS: usize = 10;

/// Every listing summary the catalog holds, paging until the service reports no more.
async fn whole_catalog_paged(router: &MultiServiceRouter) -> Vec<Value> {
    let mut summaries: Vec<Value> = Vec::new();
    let mut token = String::new();
    loop {
        let mut input = json!({ "maxResults": 100 });
        if !token.is_empty() {
            input["nextToken"] = Value::String(token.clone());
        }
        let page = ok(router, "searchListings", &input).await;
        summaries.extend(array(&page, "listingSummaries"));
        token = text(&page, "nextToken");
        if token.is_empty() {
            break;
        }
    }
    assert!(
        summaries.len() > 25,
        "case setup: the catalog must hold more listings than one default page, got {}",
        summaries.len()
    );
    summaries
}

/// The key features one record states, read off a `GetListing` or `GetProduct` answer.
fn key_features(record: &Value) -> Vec<String> {
    array(record, "highlights")
        .iter()
        .map(|feature| feature.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The fulfillment option types a record reports, which is the route — or routes — it is
/// delivered through.
fn routes(record: &Value) -> BTreeSet<String> {
    array(record, "fulfillmentOptionSummaries")
        .iter()
        .map(|summary| text(summary, "fulfillmentOptionType"))
        .collect()
}

/// A sample of the catalog's listing summaries, spanning its tiers.
fn sampled(page: &[Value]) -> Vec<Value> {
    page.iter()
        .step_by((page.len() / SAMPLED_LISTINGS).max(1))
        .cloned()
        .collect()
}

/// RULE: a record's key features are its OWN, so no two records of the catalog state the
/// same features — in particular two records delivered through the same route do not restate
/// one list.
///
/// Rows: every listing the catalog holds, across all three seeding tiers; they differ along
/// what the rule generalises over — the subject the record is classified at, the route it is
/// delivered through, how many routes it offers and which tier wrote it.
#[tokio::test]
async fn no_two_records_state_the_same_key_features() {
    let router = router();

    // Which record states each feature list, and which records share a delivery route.
    let mut stating: BTreeMap<Vec<String>, String> = BTreeMap::new();
    let mut per_route: BTreeMap<String, usize> = BTreeMap::new();
    for summary in whole_catalog_paged(&router).await {
        let listing_id = text(&summary, "listingId");
        let listing = ok(
            &router,
            "getListing",
            &json!({ "listingId": listing_id.clone() }),
        )
        .await;
        let features = key_features(&listing);
        assert!(
            !features.is_empty(),
            "case {listing_id}: a record states the key features it offers, got none in \
             {listing}"
        );
        let route = routes(&listing)
            .into_iter()
            .collect::<Vec<String>>()
            .join("+");
        *per_route.entry(route.clone()).or_default() += 1;
        if let Some(held) = stating.insert(features.clone(), listing_id.clone()) {
            panic!(
                "case {listing_id} (fulfilled as {route}): expected key features of its own, \
                 got the list {held} already states: {features:?}"
            );
        }
    }

    // The rule is asserted where it bites: several records of the catalog are delivered
    // through the same route, so a list derived from the route would have collided above.
    let shared = per_route
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(route, count)| format!("{route}: {count}"))
        .collect::<Vec<String>>();
    assert!(
        shared.len() > 2,
        "case setup: several routes must be shared by more than one record, so a \
         route-derived feature list would collide, got {per_route:?}"
    );
}

/// RULE: a detail view answers from the record the requested id resolves to — its own key
/// features, categories, prose, publisher, resources, engagements and associated product —
/// and reading it again answers the same, because the content is held rather than composed
/// per call.
///
/// Rows: a sample of listings spanning the catalog's tiers, each compared with the NEXT
/// record of the sample, so every content member is checked both for what it holds and for
/// belonging to the id it was asked for.
#[tokio::test]
async fn a_detail_view_restates_the_record_the_requested_id_names() {
    let router = router();
    let page = whole_catalog_paged(&router).await;
    let sampled = sampled(&page);
    assert!(
        sampled.len() > 2,
        "case setup: the sample must hold several records, got {}",
        sampled.len()
    );

    // The content members a record owns: a detail view reports each off the record the id
    // names, and no two records of the sample report the same values for all of them.
    const OWN_MEMBERS: &[&str] = &[
        "listingName",
        "shortDescription",
        "longDescription",
        "categories",
        "highlights",
        "publisher",
        "resources",
        "sellerEngagements",
        "useCases",
        "promotionalMedia",
        "associatedEntities",
    ];

    let mut read: Vec<(String, Value)> = Vec::new();
    for summary in &sampled {
        let listing_id = text(summary, "listingId");
        let listing = ok(
            &router,
            "getListing",
            &json!({ "listingId": listing_id.clone() }),
        )
        .await;
        assert_eq!(
            text(&listing, "listingId"),
            listing_id,
            "case {listing_id}: the answer must name the listing that was asked for, got \
             {listing}"
        );
        for member in OWN_MEMBERS {
            assert!(
                listing.get(*member).is_some(),
                "case {listing_id}: the detail view reports {member} off the record, got \
                 {listing}"
            );
        }
        // Read again: a reporting operation answers from held state, so the second read
        // restates the first.
        let again = ok(&router, "getListing", &json!({ "listingId": listing_id.clone() })).await;
        assert_eq!(
            again, listing,
            "case {listing_id}: a second read must restate the same record"
        );

        // The product the listing associates is the record that product id names, and it
        // points back at this listing.
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
            assert_eq!(
                text(&detail, "productId"),
                product_id,
                "case {product_id}: the answer must name the product that was asked for, got \
                 {detail}"
            );
            assert_eq!(
                text(&detail, "listingId"),
                listing_id,
                "case {product_id}: the product must name the listing it belongs to, got \
                 {detail}"
            );
            assert_eq!(
                text(&detail, "productName"),
                text(product, "productName"),
                "case {product_id}: the detail view and the listing's association must report \
                 one product record, got {detail}"
            );
            assert_eq!(
                detail.get("manufacturer"),
                product.get("manufacturer"),
                "case {product_id}: the detail view and the listing's association must report \
                 one seller, got {detail}"
            );
            assert_eq!(
                key_features(&detail),
                key_features(&listing),
                "case {product_id}: the product of a single-record listing states that \
                 record's features, got {detail}"
            );
        }
        read.push((listing_id, listing));
    }

    // Two different ids resolve to two different records: a view keyed on anything but the
    // id — the route, the kind of product, the first row of the catalog — would report one
    // of them twice.
    for pair in read.windows(2) {
        let [(first_id, first), (second_id, second)] = pair else {
            continue;
        };
        let differing: Vec<&str> = OWN_MEMBERS
            .iter()
            .copied()
            .filter(|member| first.get(*member) != second.get(*member))
            .collect();
        assert!(
            differing.len() >= 5,
            "case {first_id} against {second_id}: two records must differ in the content they \
             own, got only {differing:?} differing"
        );
        for member in ["listingName", "highlights", "categories"] {
            assert_ne!(
                first.get(member),
                second.get(member),
                "case {first_id} against {second_id}: {member} is the record's own, got the \
                 same value for both"
            );
        }
    }
}

/// RULE: every SaaS fulfillment option names the deployment templates it offers — a present
/// list, empty where the catalog holds no template for it — and no option of another type
/// carries that member, which belongs to the SaaS arm.
///
/// Rows: every product of a sample of listings spanning the catalog's tiers: products
/// fulfilled through one route and through several, SaaS options with quick launch enabled
/// and disabled, and options of every other arm the catalog offers.
#[tokio::test]
async fn every_saas_option_names_the_deployment_templates_it_offers() {
    let router = router();
    let page = whole_catalog_paged(&router).await;

    // The rows: a sample spanning the tiers, plus every listing a product of which supports
    // quick launch, so a SaaS option of each quick-launch status is covered — that status is
    // what decides whether a seller offers deployment templates at all.
    let mut rows = sampled(&page);
    for summary in &page {
        let quick_launchable = array(summary, "badges")
            .iter()
            .any(|badge| text(badge, "badgeType") == "QUICK_LAUNCH");
        if quick_launchable {
            rows.push(summary.clone());
        }
    }

    let mut saas_options = 0_usize;
    let mut other_options = 0_usize;
    let mut quick_launch: BTreeSet<String> = BTreeSet::new();
    for summary in rows {
        let listing_id = text(&summary, "listingId");
        let listing = ok(
            &router,
            "getListing",
            &json!({ "listingId": listing_id.clone() }),
        )
        .await;
        for entity in array(&listing, "associatedEntities") {
            let Some(product) = entity.get("product") else {
                continue;
            };
            let product_id = text(product, "productId");
            let body = ok(
                &router,
                "listFulfillmentOptions",
                &json!({ "productId": product_id.clone(), "maxResults": 50 }),
            )
            .await;
            let options = array(&body, "fulfillmentOptions");
            assert!(
                !options.is_empty(),
                "case {product_id}: a catalog product reports how it is fulfilled, got {body}"
            );
            for option in &options {
                let object = option
                    .as_object()
                    .unwrap_or_else(|| panic!("case {product_id}: an option is an object"));
                let (member, served) = object
                    .iter()
                    .next()
                    .unwrap_or_else(|| panic!("case {product_id}: an option carries a member"));
                if member != SAAS_MEMBER {
                    assert!(
                        served.get(DEPLOYMENT_TEMPLATES).is_none(),
                        "case {product_id} {member}: {DEPLOYMENT_TEMPLATES} belongs to the \
                         SaaS option alone, got {served}"
                    );
                    other_options += 1;
                    continue;
                }
                let templates = served.get(DEPLOYMENT_TEMPLATES);
                assert_eq!(
                    templates.and_then(Value::as_array),
                    Some(&Vec::new()),
                    "case {product_id}: a SaaS option names the deployment templates it \
                     offers — a present, empty list where the catalog holds none — got \
                     {templates:?} in {served}"
                );
                // The member is written beside everything the option already reported, so a
                // client still reads the option's own identity and addresses.
                for named in ["fulfillmentOptionId", "fulfillmentOptionType", "quickLaunch"] {
                    assert!(
                        !text(served, named).is_empty(),
                        "case {product_id}: {named} must survive beside the templates member, \
                         got {served}"
                    );
                }
                quick_launch.insert(text(served, "quickLaunch"));
                saas_options += 1;
            }
        }
    }

    assert!(
        saas_options > 2 && other_options > 2,
        "case setup: the sample must cover several SaaS options and several options of other \
         arms, got {saas_options} and {other_options}"
    );
    assert!(
        quick_launch.len() > 1,
        "case setup: SaaS options with quick launch both enabled and disabled must be \
         covered, got {quick_launch:?}"
    );
}
