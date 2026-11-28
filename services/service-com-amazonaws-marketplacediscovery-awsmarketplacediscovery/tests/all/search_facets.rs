// SPDX-License-Identifier: CC0-1.0

//! `SearchFacets` over the seeded AWS Marketplace Discovery catalog.
//!
//! The rule under test: for the listings a `searchText`/`filters` query matches,
//! `SearchFacets` reports that match set's size as `totalResults` and, for each requested
//! facet type, one facet value per distinct value present in the set with the count of
//! matching listings carrying it — so filtering a search on a facet value returns exactly
//! that value's count, and `totalResults` agrees with `SearchListings` for the same query.
//!
//! Every suite drives real HTTP requests through the `MultiServiceRouter`, so the protocol,
//! the routing table, input validation and the handlers are all exercised.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::ops::types::{
    SEARCH_FACET_TYPE_VALUES, SEARCH_FILTER_TYPE_VALUES,
};
use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::state::category_id;
use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::EmulatedService;

/// The model's `POST /2026-02-05/<op>` route prefix (`OPERATIONS.md`).
const PREFIX: &str = "http://discovery-marketplace.us-east-1.amazonaws.com/2026-02-05";

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name, which is what the router's `can_handle` routes on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260205/us-east-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// Every facet type the service computes, paired with the search filter its values feed
/// back into.
///
/// The facet types are checked against the model-generated `SEARCH_FACET_TYPE_VALUES` by
/// `the_facet_type_table_covers_the_model` below, so this table cannot silently fall behind
/// the `SearchFacetType` enum. `AVERAGE_CUSTOMER_RATING` is the one facet type with no
/// same-named `SearchFilterType`: its values are minimum ratings, which
/// `MIN_AVERAGE_CUSTOMER_RATING` takes.
const FACET_TYPES: [(&str, &str); 13] = [
    ("AVERAGE_CUSTOMER_RATING", "MIN_AVERAGE_CUSTOMER_RATING"),
    ("CATEGORY", "CATEGORY"),
    ("PUBLISHER", "PUBLISHER"),
    ("FULFILLMENT_OPTION_TYPE", "FULFILLMENT_OPTION_TYPE"),
    ("PRICING_MODEL", "PRICING_MODEL"),
    ("PRICING_UNIT", "PRICING_UNIT"),
    ("DEPLOYED_ON_AWS", "DEPLOYED_ON_AWS"),
    ("NUMBER_OF_PRODUCTS", "NUMBER_OF_PRODUCTS"),
    ("CERTIFICATES", "CERTIFICATES"),
    ("SELLER_ENGAGEMENT", "SELLER_ENGAGEMENT"),
    ("SPECIALIZATION", "SPECIALIZATION"),
    ("SPECIALIZATION_CATEGORY", "SPECIALIZATION_CATEGORY"),
    ("TRUST_CENTER", "TRUST_CENTER"),
];

fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite signs nothing, so request authentication is off for this router only.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, input: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("{PREFIX}/{operation}"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(input.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, body)
}

/// Call one operation, requiring HTTP 200.
async fn ok(router: &MultiServiceRouter, operation: &str, input: &Value) -> Value {
    let (status, body) = call(router, operation, input).await;
    assert_eq!(
        status, 200,
        "case {operation} {input}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

fn text(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn count(value: &Value, member: &str) -> i64 {
    value.get(member).and_then(Value::as_i64).unwrap_or(-1)
}

/// The facet lists a `SearchFacets` response carries, keyed by facet type.
fn listing_facets(body: &Value) -> Vec<(String, Vec<Value>)> {
    body.get("listingFacets")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(facet_type, facets)| {
                    (
                        facet_type.clone(),
                        facets.as_array().cloned().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn facet_type_keys(body: &Value) -> BTreeSet<String> {
    listing_facets(body)
        .into_iter()
        .map(|(facet_type, _)| facet_type)
        .collect()
}

/// A `SearchListings` input for the same criteria as a `SearchFacets` input, with `filters`
/// extended by `extra`.
fn listings_input(query: &Value, extra: Option<Value>) -> Value {
    let mut input = json!({ "maxResults": 100 });
    if let Some(search_text) = query.get("searchText") {
        input["searchText"] = search_text.clone();
    }
    let mut filters = query
        .get("filters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(extra) = extra {
        filters.push(extra);
    }
    if !filters.is_empty() {
        input["filters"] = Value::Array(filters);
    }
    input
}

/// Whether `query` already filters on `filter_type`.
///
/// A request names a `filterType` at most once — OR-combination belongs inside one entry's
/// `filterValues` (`SearchFilter$filterValues`), so the service refuses a repeated type —
/// which means a query already filtering on a facet's own filter type cannot be refined by
/// that facet's values in ONE further request.
fn filters_on(query: &Value, filter_type: &str) -> bool {
    query
        .get("filters")
        .and_then(Value::as_array)
        .is_some_and(|filters| {
            filters
                .iter()
                .any(|filter| text(filter, "filterType") == filter_type)
        })
}

/// The queries the facet rules are asserted over: no criteria at all, text that matches
/// many / one / nothing, a term filter, a range filter, two filters at once, text and a
/// filter together, and a filter nothing matches.
fn queries() -> Vec<(&'static str, Value)> {
    vec![
        ("whole catalog", json!({})),
        ("searchText=a", json!({ "searchText": "a" })),
        ("searchText=analytics", json!({ "searchText": "analytics" })),
        ("searchText=firewall", json!({ "searchText": "firewall" })),
        (
            "searchText=no-such-listing",
            json!({ "searchText": "no-such-listing" }),
        ),
        (
            "CATEGORY=machine-learning",
            json!({ "filters": [{ "filterType": "CATEGORY", "filterValues": [category_id("machine-learning")] }] }),
        ),
        (
            "PRICING_MODEL=CONTRACT|USAGE",
            json!({ "filters": [{ "filterType": "PRICING_MODEL", "filterValues": ["CONTRACT", "USAGE"] }] }),
        ),
        (
            "MIN_AVERAGE_CUSTOMER_RATING=4.0",
            json!({ "filters": [{ "filterType": "MIN_AVERAGE_CUSTOMER_RATING", "filterValues": ["4.0"] }] }),
        ),
        (
            "NUMBER_OF_PRODUCTS=1 and DEPLOYED_ON_AWS=DEPLOYED",
            json!({ "filters": [
                { "filterType": "NUMBER_OF_PRODUCTS", "filterValues": ["1"] },
                { "filterType": "DEPLOYED_ON_AWS", "filterValues": ["DEPLOYED"] },
            ] }),
        ),
        (
            "searchText=data and PUBLISHER filter",
            json!({
                "searchText": "data",
                "filters": [{ "filterType": "PUBLISHER", "filterValues": ["sp-acme", "sp-orion"] }],
            }),
        ),
        (
            "CATEGORY=no-such-category",
            json!({ "filters": [{ "filterType": "CATEGORY", "filterValues": ["no-such-category"] }] }),
        ),
    ]
}

/// The facet-type table this suite asserts over is exactly the model's `SearchFacetType`
/// enum, and every filter it maps a facet value onto is a `SearchFilterType`.
#[test]
fn the_facet_type_table_covers_the_model() {
    let tabled: BTreeSet<&str> = FACET_TYPES
        .iter()
        .map(|(facet_type, _)| *facet_type)
        .collect();
    let modelled: BTreeSet<&str> = SEARCH_FACET_TYPE_VALUES.iter().copied().collect();
    assert_eq!(
        tabled, modelled,
        "case SearchFacetType: the suite must cover every modelled facet type, got {tabled:?}"
    );
    for (facet_type, filter_type) in FACET_TYPES {
        assert!(
            SEARCH_FILTER_TYPE_VALUES.contains(&filter_type),
            "case {facet_type}: {filter_type} must be a modelled SearchFilterType, which are {SEARCH_FILTER_TYPE_VALUES:?}"
        );
    }
}

/// Every filter type the model declares is one the search operations support: none falls
/// through to the "not a supported filterType" refusal.
#[tokio::test]
async fn every_modelled_filter_type_is_supported() {
    let router = router();
    for filter_type in SEARCH_FILTER_TYPE_VALUES {
        // A value each filter type accepts: a rating for the range filters, a count for
        // NUMBER_OF_PRODUCTS, and an id no listing carries for the term filters — what
        // matters here is that the filter TYPE is understood.
        let value = match *filter_type {
            "MIN_AVERAGE_CUSTOMER_RATING" | "MAX_AVERAGE_CUSTOMER_RATING" => "3.0",
            "NUMBER_OF_PRODUCTS" => "1",
            _ => "no-such-value",
        };
        let input =
            json!({ "filters": [{ "filterType": filter_type, "filterValues": [value] }] });
        for operation in ["searchFacets", "searchListings"] {
            let mut input = input.clone();
            if operation == "searchListings" {
                input["maxResults"] = json!(100);
            }
            let (status, body) = call(&router, operation, &input).await;
            assert_eq!(
                status, 200,
                "case {operation} {filter_type}={value}: a modelled filter type must be supported, got {status} with {body}"
            );
        }
    }
}

/// `totalResults` is the size of the match set, which is the one `SearchListings` reports
/// for the same `searchText` and `filters`.
#[tokio::test]
async fn total_results_is_the_same_match_set_search_listings_reports() {
    let router = router();
    let mut matched_something = false;
    for (case, query) in queries() {
        let facets = ok(&router, "searchFacets", &query).await;
        let listings = ok(&router, "searchListings", &listings_input(&query, None)).await;
        let expected = count(&listings, "totalResults");
        assert_eq!(
            count(&facets, "totalResults"),
            expected,
            "case {case}: SearchFacets totalResults must equal the SearchListings total for the same query, got {facets} against {listings}"
        );
        matched_something |= expected > 0;
    }
    assert!(
        matched_something,
        "case queries(): at least one query must match a listing, or the suite asserts nothing"
    );
}

/// Filtering a search on a facet value returns exactly that value's count, for every facet
/// type and every query.
#[tokio::test]
async fn every_facet_count_is_what_filtering_a_search_on_that_value_returns() {
    let router = router();
    let mut checked = 0_usize;
    for (case, query) in queries() {
        let facets = ok(&router, "searchFacets", &query).await;
        let total = count(&facets, "totalResults");
        for (facet_type, facet_list) in listing_facets(&facets) {
            let filter_type = FACET_TYPES
                .iter()
                .find(|(known, _)| *known == facet_type)
                .map(|(_, filter_type)| *filter_type)
                .unwrap_or_else(|| panic!("case {case}: unexpected facet type {facet_type}"));
            let mut seen: BTreeSet<String> = BTreeSet::new();
            for facet in &facet_list {
                let value = text(facet, "value");
                let row = format!("{case} / {facet_type}={value}");
                assert!(
                    !value.is_empty(),
                    "case {row}: a facet value must be non-empty, got {facet}"
                );
                assert!(
                    !text(facet, "displayName").trim().is_empty(),
                    "case {row}: a facet displayName must be non-blank, got {facet}"
                );
                assert!(
                    seen.insert(value.clone()),
                    "case {row}: a facet value must appear once per facet type, got {facet_list:?}"
                );
                let facet_count = count(facet, "count");
                assert!(
                    facet_count >= 1 && facet_count <= total,
                    "case {row}: expected a count in 1..={total}, got {facet_count}"
                );

                // A `filterType` may be named once per request (OR-combination belongs
                // inside one entry's `filterValues`), so a query that already filters on
                // this facet's own filter type is REFUSED when refined by it rather than
                // having the two entries ANDed.
                if filters_on(&query, filter_type) {
                    let (status, body) = call(
                        &router,
                        "searchListings",
                        &listings_input(
                            &query,
                            Some(json!({ "filterType": filter_type, "filterValues": [value] })),
                        ),
                    )
                    .await;
                    assert_eq!(
                        status, 400,
                        "case {row}: refining a query by a filter type it already names must be \
                         refused, got {status} with {body}"
                    );
                    assert_eq!(
                        text(&body, "message"),
                        format!("Duplicate filter types: [{filter_type}]"),
                        "case {row}: expected the duplicate-filter-type refusal, got {body}"
                    );
                    continue;
                }

                let filtered = ok(
                    &router,
                    "searchListings",
                    &listings_input(
                        &query,
                        Some(json!({ "filterType": filter_type, "filterValues": [value] })),
                    ),
                )
                .await;
                assert_eq!(
                    count(&filtered, "totalResults"),
                    facet_count,
                    "case {row}: a search filtered on the facet value must return its count, got {filtered}"
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked > FACET_TYPES.len(),
        "case queries(): expected facet values across every facet type, checked only {checked}"
    );
}

/// A query nothing matches reports no facet values at all, and does not fail.
#[tokio::test]
async fn a_query_with_no_matches_reports_every_facet_type_empty() {
    let router = router();
    for (case, query) in [
        (
            "searchText=no-such-listing",
            json!({ "searchText": "no-such-listing" }),
        ),
        (
            "CATEGORY=no-such-category",
            json!({ "filters": [{ "filterType": "CATEGORY", "filterValues": ["no-such-category"] }] }),
        ),
    ] {
        let facets = ok(&router, "searchFacets", &query).await;
        assert_eq!(
            count(&facets, "totalResults"),
            0,
            "case {case}: a query nothing matches has no results, got {facets}"
        );
        for (facet_type, facet_list) in listing_facets(&facets) {
            assert!(
                facet_list.is_empty(),
                "case {case} / {facet_type}: no listing matches, so no facet value is available, got {facet_list:?}"
            );
        }
    }
}

/// The categories of the matching listings are reported with positive counts, and each one
/// is a `CATEGORY` filter value that narrows the search to those listings.
#[tokio::test]
async fn the_category_facet_reports_the_categories_of_the_matching_listings() {
    let router = router();
    let facets = ok(&router, "searchFacets", &json!({ "facetTypes": ["CATEGORY"] })).await;
    let categories = listing_facets(&facets)
        .into_iter()
        .find(|(facet_type, _)| facet_type == "CATEGORY")
        .map(|(_, facets)| facets)
        .unwrap_or_default();
    assert!(
        categories.len() > 1,
        "case whole catalog: the seeded catalog spans several categories, got {categories:?}"
    );

    // Every category a listing summary carries is reported, with the number of listings
    // carrying it.
    let listings = ok(&router, "searchListings", &json!({ "maxResults": 100 })).await;
    let summaries = listings
        .get("listingSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut expected: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for summary in &summaries {
        for category in summary
            .get("categories")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            *expected.entry(text(&category, "categoryId")).or_insert(0) += 1;
        }
    }
    let actual: std::collections::BTreeMap<String, i64> = categories
        .iter()
        .map(|facet| (text(facet, "value"), count(facet, "count")))
        .collect();
    assert_eq!(
        actual, expected,
        "case whole catalog: the CATEGORY facet must count the listings in each category, got {categories:?}"
    );
}

/// `listingFacets` carries the requested facet types, and every facet type when the request
/// names none (`SearchFacetsInput$facetTypes`).
#[tokio::test]
async fn listing_facets_carries_exactly_the_requested_facet_types() {
    let router = router();
    let all: BTreeSet<String> = FACET_TYPES
        .iter()
        .map(|(facet_type, _)| (*facet_type).to_string())
        .collect();
    let cases: Vec<(&str, Value, BTreeSet<String>)> = vec![
        ("facetTypes absent", json!({}), all.clone()),
        ("facetTypes empty", json!({ "facetTypes": [] }), all.clone()),
        (
            "facetTypes=[CATEGORY]",
            json!({ "facetTypes": ["CATEGORY"] }),
            ["CATEGORY"].iter().map(|t| (*t).to_string()).collect(),
        ),
        (
            "facetTypes=[PRICING_UNIT, PUBLISHER]",
            json!({ "facetTypes": ["PRICING_UNIT", "PUBLISHER"] }),
            ["PRICING_UNIT", "PUBLISHER"]
                .iter()
                .map(|t| (*t).to_string())
                .collect(),
        ),
        (
            "facetTypes=[CATEGORY, CATEGORY]",
            json!({ "facetTypes": ["CATEGORY", "CATEGORY"] }),
            ["CATEGORY"].iter().map(|t| (*t).to_string()).collect(),
        ),
        (
            "facetTypes=every facet type",
            json!({ "facetTypes": all.iter().cloned().collect::<Vec<String>>() }),
            all.clone(),
        ),
    ];
    for (case, input, expected) in cases {
        let facets = ok(&router, "searchFacets", &input).await;
        assert_eq!(
            facet_type_keys(&facets),
            expected,
            "case {case}: expected exactly those facet types, got {facets}"
        );
        // Scoping the request changes WHICH facets come back, never how many listings
        // matched.
        assert!(
            count(&facets, "totalResults") > 0,
            "case {case}: the unfiltered catalog matches listings, got {facets}"
        );
    }
}

/// A request the service cannot answer is refused with HTTP 400 `ValidationException`,
/// never with an empty or partial success.
#[tokio::test]
async fn an_unanswerable_request_is_refused() {
    let router = router();

    // A token minted for another query is not a position in this one.
    let paged = ok(
        &router,
        "searchListings",
        &json!({ "maxResults": 1, "searchText": "a" }),
    )
    .await;
    let foreign_token = text(&paged, "nextToken");
    assert!(
        !foreign_token.is_empty(),
        "case setup: SearchListings must page the catalog at maxResults=1, got {paged}"
    );

    let cases: Vec<(&str, Value)> = vec![
        (
            "facetTypes=[NOT_A_FACET_TYPE]",
            json!({ "facetTypes": ["NOT_A_FACET_TYPE"] }),
        ),
        (
            "facetTypes=[category] (wrong case)",
            json!({ "facetTypes": ["category"] }),
        ),
        (
            "facetTypes=[CATEGORY, NOT_A_FACET_TYPE]",
            json!({ "facetTypes": ["CATEGORY", "NOT_A_FACET_TYPE"] }),
        ),
        ("locale=xx", json!({ "locale": "xx" })),
        (
            "MIN_AVERAGE_CUSTOMER_RATING=7.0",
            json!({ "filters": [{ "filterType": "MIN_AVERAGE_CUSTOMER_RATING", "filterValues": ["7.0"] }] }),
        ),
        (
            "MIN_AVERAGE_CUSTOMER_RATING with two values",
            json!({ "filters": [{ "filterType": "MIN_AVERAGE_CUSTOMER_RATING", "filterValues": ["1.0", "2.0"] }] }),
        ),
        (
            "NUMBER_OF_PRODUCTS=not-a-number",
            json!({ "filters": [{ "filterType": "NUMBER_OF_PRODUCTS", "filterValues": ["not-a-number"] }] }),
        ),
        // A malformed filter is refused for what the REQUEST says, so a searchText that
        // matches no listing does not turn the refusal into an empty success.
        (
            "MIN_AVERAGE_CUSTOMER_RATING=7.0 with searchText nothing matches",
            json!({
                "searchText": "no-such-listing",
                "filters": [{ "filterType": "MIN_AVERAGE_CUSTOMER_RATING", "filterValues": ["7.0"] }],
            }),
        ),
        (
            "nextToken minted by SearchListings",
            json!({ "nextToken": foreign_token }),
        ),
        (
            "nextToken this service never minted",
            json!({ "nextToken": "bm90LWEtdG9rZW4=" }),
        ),
    ];
    for (case, input) in cases {
        let (status, body) = call(&router, "searchFacets", &input).await;
        assert_eq!(
            status, 400,
            "case {case}: an unanswerable request must be refused, got {status} with {body}"
        );
        assert_eq!(
            text(&body, "__type"),
            "ValidationException",
            "case {case}: expected a ValidationException, got {body}"
        );
    }
}
