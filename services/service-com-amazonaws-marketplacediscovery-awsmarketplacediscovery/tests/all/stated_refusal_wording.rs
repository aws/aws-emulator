// SPDX-License-Identifier: CC0-1.0

//! What a constraint refusal of this service STATES: the subject it quotes, the position it
//! names and the value set it lists.
//!
//! Three rules, each asserted over a TABLE that spans what the rule generalises over rather
//! than over the one request a report cited:
//!
//! 1. **A constraint an ELEMENT of a collection breaks is stated at the COLLECTION member.**
//!    The clause names the collection — no position, no `member` step — echoes the WHOLE
//!    list the caller sent, rendered `[<element>, …]`, and nests the element's requirement
//!    inside `Member must satisfy constraint: [ … ]`. A constraint the collection ITSELF
//!    breaks (its own `@length`) is not nested, and neither is one a member INSIDE a
//!    structure element breaks, which keeps its own full path.
//! 2. **A clause echoes the value the caller SUBMITTED, in full.** However long the value,
//!    the echo is the caller's own bytes with nothing elided and no marker appended: this
//!    message is the only place the server reports the offending input back.
//! 3. **An enum clause lists the value set as this SERVICE states it** — fixed text of the
//!    refusal, carrying nothing of the request, stated the same way at every member that
//!    enum constrains and on every operation. Where a recorded refusal predates a value the
//!    model has since gained, the recorded order is kept for the values it covered and the
//!    rest follow it in the model's own order.
//!
//! Every request goes through the `MultiServiceRouter`, so the protocol, the routing table
//! and the framework's input walk are all exercised end to end.

use serde_json::{json, Value};

use crate::catalog_drilldown::{call, router};
use crate::validation_message_rules::refused;

/// The value set this service STATES for `SearchFacetType`: the nine values the recorded
/// refusal listed — among them `CERTIFICATES`, which the model did not declare when that
/// refusal was recorded — in its own order, then the four values the model has gained since,
/// in the model's declaration order. Fixed text of the refusal.
const STATED_SEARCH_FACET_TYPES: &str = "FULFILLMENT_OPTION_TYPE, PRICING_MODEL, \
     NUMBER_OF_PRODUCTS, DEPLOYED_ON_AWS, CERTIFICATES, PUBLISHER, AVERAGE_CUSTOMER_RATING, \
     PRICING_UNIT, CATEGORY, SELLER_ENGAGEMENT, SPECIALIZATION, SPECIALIZATION_CATEGORY, \
     TRUST_CENTER";

/// The value set this service STATES for `PurchaseOptionFilterType`: the five values the
/// model declares, in this service's own order.
const STATED_PURCHASE_OPTION_FILTER_TYPES: &str = "PURCHASE_OPTION_TYPE, VISIBILITY_SCOPE, \
     PRODUCT_ID, AVAILABILITY_STATUS, SELLER_OF_RECORD_PROFILE_ID";

/// The value set this service STATES for `SearchFilterType`: the nine values the recorded
/// refusal listed, in its own order, then the six the model has gained since, in the model's
/// declaration order.
const STATED_SEARCH_FILTER_TYPES: &str = "MAX_AVERAGE_CUSTOMER_RATING, \
     FULFILLMENT_OPTION_TYPE, MIN_AVERAGE_CUSTOMER_RATING, PRICING_MODEL, NUMBER_OF_PRODUCTS, \
     DEPLOYED_ON_AWS, PUBLISHER, PRICING_UNIT, CATEGORY, CERTIFICATES, PRODUCT_ID, \
     SELLER_ENGAGEMENT, SPECIALIZATION, SPECIALIZATION_CATEGORY, TRUST_CENTER";

/// The one-violation envelope AWS reports a single constraint failure in.
fn one_violation(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// Rule 1 over the collections this service's inputs declare: a `facetTypes` element outside
/// its enum, a `filterValues` element outside its `@pattern` and outside its `@length`, at
/// several positions and list lengths, and on both operations that carry `filters`.
///
/// Each row is `(case, operation, input, the collection path, the whole list as echoed, the
/// element requirement nested in the clause)`.
#[allow(clippy::type_complexity)]
fn element_cases() -> Vec<(&'static str, &'static str, Value, &'static str, String, String)> {
    let enum_requirement =
        format!("Member must satisfy enum value set: [{STATED_SEARCH_FACET_TYPES}]");
    let long = "v".repeat(129);
    vec![
        (
            "a one-element facetTypes list outside SearchFacetType",
            "searchFacets",
            json!({ "facetTypes": ["NOT_A_REAL_FACET_TYPE"] }),
            "facetTypes",
            "[NOT_A_REAL_FACET_TYPE]".to_string(),
            enum_requirement.clone(),
        ),
        (
            "the SECOND element of facetTypes outside the enum, the first inside it",
            "searchFacets",
            json!({ "facetTypes": ["CATEGORY", "NOT_A_REAL_FACET_TYPE"] }),
            "facetTypes",
            "[CATEGORY, NOT_A_REAL_FACET_TYPE]".to_string(),
            enum_requirement.clone(),
        ),
        (
            "two bad elements of facetTypes, which is ONE clause",
            "searchFacets",
            json!({ "facetTypes": ["category", "CATEGORIES"] }),
            "facetTypes",
            "[category, CATEGORIES]".to_string(),
            enum_requirement,
        ),
        (
            "a filterValues element outside its @pattern, on SearchListings",
            "searchListings",
            json!({
                "maxResults": 5,
                "filters": [{ "filterType": "CATEGORY", "filterValues": ["security", "not an id!"] }],
            }),
            "filters.1.member.filterValues",
            "[security, not an id!]".to_string(),
            r"Member must satisfy regular expression pattern: [\w\-\.]+".to_string(),
        ),
        (
            "a filterValues element past its @length, on SearchFacets",
            "searchFacets",
            json!({
                "filters": [{ "filterType": "PUBLISHER", "filterValues": [long.clone()] }],
            }),
            "filters.1.member.filterValues",
            format!("[{long}]"),
            "Member must have length less than or equal to 128".to_string(),
        ),
        (
            "a filterValues element outside its @pattern, on ListPurchaseOptions",
            "listPurchaseOptions",
            json!({
                "filters": [{ "filterType": "PRODUCT_ID", "filterValues": ["not an id!"] }],
            }),
            "filters.1.member.filterValues",
            "[not an id!]".to_string(),
            r"Member must satisfy regular expression pattern: [\w\-]+".to_string(),
        ),
    ]
}

#[tokio::test]
async fn an_element_constraint_is_stated_at_its_collection_with_the_whole_list_echoed() {
    let router = router();
    for (case, operation, input, path, echoed, requirement) in element_cases() {
        let message = refused(&router, case, operation, &input).await;
        let expected = one_violation(&format!(
            "Value '{echoed}' at '{path}' failed to satisfy constraint: \
             Member must satisfy constraint: [{requirement}]"
        ));
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// Rule 1's other half: a constraint the COLLECTION itself breaks is stated at that
/// collection WITHOUT the nested form, and a constraint a member INSIDE a structure element
/// breaks keeps its own full path.
#[tokio::test]
async fn a_collections_own_constraint_and_a_members_are_not_nested() {
    let router = router();

    // Rows: a list below its own `@length` minimum on each operation that declares one, and
    // an enum member inside a structure element of a list.
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "an empty filters list on ListPurchaseOptions",
            "listPurchaseOptions",
            json!({ "filters": [] }),
            "Value '0 items' at 'filters' failed to satisfy constraint: \
             Member must have length greater than or equal to 1"
                .to_string(),
        ),
        (
            "an empty filters list on SearchListings",
            "searchListings",
            json!({ "maxResults": 5, "filters": [] }),
            "Value '0 items' at 'filters' failed to satisfy constraint: \
             Member must have length greater than or equal to 1"
                .to_string(),
        ),
        (
            "a filterType inside a filters element, outside PurchaseOptionFilterType",
            "listPurchaseOptions",
            json!({
                "filters": [{ "filterType": "NOT_A_REAL_FILTER_TYPE", "filterValues": ["x"] }],
            }),
            format!(
                "Value 'NOT_A_REAL_FILTER_TYPE' at 'filters.1.member.filterType' failed to \
                 satisfy constraint: Member must satisfy enum value set: \
                 [{STATED_PURCHASE_OPTION_FILTER_TYPES}]"
            ),
        ),
        (
            "a filterType inside the SECOND filters element, outside SearchFilterType",
            "searchListings",
            json!({
                "maxResults": 5,
                "filters": [
                    { "filterType": "CATEGORY", "filterValues": ["security"] },
                    { "filterType": "NOT_A_REAL_FILTER_TYPE", "filterValues": ["x"] },
                ],
            }),
            format!(
                "Value 'NOT_A_REAL_FILTER_TYPE' at 'filters.2.member.filterType' failed to \
                 satisfy constraint: Member must satisfy enum value set: \
                 [{STATED_SEARCH_FILTER_TYPES}]"
            ),
        ),
    ];
    for (case, operation, input, clause) in cases {
        let message = refused(&router, case, operation, &input).await;
        let expected = one_violation(&clause);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// Rule 2: the echo is the whole submitted value, whatever its length — and the value AT the
/// bound is still accepted, so what the rows below exercise is the refusal's rendering.
///
/// `searchText` is `@length(1..512)`
/// (`smithy_query(query="operation",operation="SearchListings")`), so the rows run from one
/// character past that bound to far past it, and across both operations that carry the
/// member. The longest row is well inside the ceiling the restoring walk states for a single
/// value (`service_common::validation`), which is what a value this emulator may echo in
/// full is bounded by.
#[tokio::test]
async fn a_refused_value_is_echoed_as_submitted_however_long() {
    let router = router();

    // Accepted AT the bound, on both operations: the boundary is not what these rows pin.
    for operation in ["searchListings", "searchFacets"] {
        let (status, body) = call(
            &router,
            operation,
            &json!({ "searchText": "x".repeat(512) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {operation} searchText at its maximum length: expected HTTP 200, got {status} \
             with {body}"
        );
    }

    // Rows: one character past the bound, a value twice it, one four times it, and one
    // carrying multi-BYTE characters, so neither a byte count nor a character count can be
    // mistaken for the other.
    let values: Vec<(&str, String)> = vec![
        ("one character past the bound", "x".repeat(513)),
        ("twice the bound", "y".repeat(1024)),
        ("four times the bound", "z".repeat(2048)),
        ("multi-byte characters past the bound", "é".repeat(600)),
        (
            "a sentence repeated past the bound",
            "cloud infrastructure ".repeat(40),
        ),
    ];
    for operation in ["searchListings", "searchFacets"] {
        for (row, value) in &values {
            let case = format!("{operation} searchText {row}");
            let mut input = json!({ "searchText": value });
            if operation == "searchListings" {
                input["maxResults"] = json!(1);
            }
            let message = refused(&router, &case, operation, &input).await;
            let expected = one_violation(&format!(
                "Value '{value}' at 'searchText' failed to satisfy constraint: \
                 Member must have length less than or equal to 512"
            ));
            assert_eq!(
                message, expected,
                "case {case:?}: the echo must be the submitted value in full, got {message:?}"
            );
            // The elision the framework's own walk renders a long value with is an emulator
            // artifact no AWS message carries.
            assert!(
                !message.contains("... ["),
                "case {case:?}: the echo must carry no elision marker, got {message:?}"
            );
        }
    }
}

/// Rule 3: every enum value set this service states, over every member and operation that
/// enum constrains — and a set this service states nothing for is still the model's.
#[tokio::test]
async fn an_enum_clause_lists_the_value_set_this_service_states() {
    let router = router();

    // Rows: (case, operation, input, the set the clause must list).
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "SearchFacets facetTypes",
            "searchFacets",
            json!({ "facetTypes": ["NOT_A_REAL_FACET_TYPE"] }),
            STATED_SEARCH_FACET_TYPES,
        ),
        (
            "ListPurchaseOptions filters.filterType",
            "listPurchaseOptions",
            json!({
                "filters": [{ "filterType": "NOT_A_REAL_FILTER_TYPE", "filterValues": ["x"] }],
            }),
            STATED_PURCHASE_OPTION_FILTER_TYPES,
        ),
        (
            "ListPurchaseOptions filters.filterType, a value of the OTHER filter enum",
            "listPurchaseOptions",
            json!({
                "filters": [{ "filterType": "CATEGORY", "filterValues": ["x"] }],
            }),
            STATED_PURCHASE_OPTION_FILTER_TYPES,
        ),
        (
            "SearchListings filters.filterType",
            "searchListings",
            json!({
                "maxResults": 5,
                "filters": [{ "filterType": "NOT_A_REAL_FILTER_TYPE", "filterValues": ["x"] }],
            }),
            STATED_SEARCH_FILTER_TYPES,
        ),
        (
            "SearchFacets filters.filterType, the same enum on another operation",
            "searchFacets",
            json!({
                "filters": [{ "filterType": "PURCHASE_OPTION_TYPE", "filterValues": ["OFFER"] }],
            }),
            STATED_SEARCH_FILTER_TYPES,
        ),
        (
            "SearchListings sortBy",
            "searchListings",
            json!({ "maxResults": 5, "sortBy": "NOT_A_SORT_KEY" }),
            // No order is recorded for `SearchListingsSortBy`, so the clause lists the
            // model's own values (`SEARCH_LISTINGS_SORT_BY_VALUES`, in declaration order).
            "RELEVANCE, AVERAGE_CUSTOMER_RATING",
        ),
        (
            "SearchListings sortOrder",
            "searchListings",
            json!({ "maxResults": 5, "sortOrder": "SIDEWAYS" }),
            // Likewise `SearchListingsSortOrder`, which the model declares `DESCENDING`
            // first (its default).
            "DESCENDING, ASCENDING",
        ),
    ];
    for (case, operation, input, set) in cases {
        let message = refused(&router, case, operation, &input).await;
        let clause = format!("Member must satisfy enum value set: [{set}]");
        assert!(
            message.contains(&clause),
            "case {case:?}: expected the clause {clause:?}, got {message:?}"
        );
    }
}

/// Rule 3 is about the REFUSAL's fixed text, not about what the service accepts: every facet
/// type the model declares is still served, so the stated set is not a set of values this
/// emulator cannot take.
#[tokio::test]
async fn every_modelled_facet_type_is_still_served() {
    let router = router();
    for facet_type in
        service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::ops::types::SEARCH_FACET_TYPE_VALUES
    {
        let (status, body) = call(
            &router,
            "searchFacets",
            &json!({ "facetTypes": [facet_type] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case facetTypes=[{facet_type}]: a modelled facet type must be served, got {status} \
             with {body}"
        );
    }
}
