// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal: the three rules its `message` follows, each over a
//! table of requests rather than over one recorded call.
//!
//! For `restJson1` the `message` is the only human-readable part of a fault, so each of
//! these is as much a requirement as the status and the error code:
//!
//! 1. a `ResourceNotFoundException` names the resource the lookup was made by, spelled
//!    as the MODEL spells it (`offerSetId` -> `OfferSet`, the spelling of `OfferSetId` and
//!    `GetOfferSet`), never re-spelled as English prose;
//! 2. an enum clause lists the admissible value set in the order THIS SERVICE emits it,
//!    one fixed order per enum, the same wherever that set appears and independent of the
//!    request;
//! 3. a `searchText` carrying a newline is refused by this service's own sentence, which
//!    names the member and carries nothing else — no validation-report envelope, no echo
//!    of the submitted value and no regular expression.
//!
//! Every request goes through the `MultiServiceRouter`, so the protocol, the routing
//! table, the framework's input walk and the handlers are all exercised end to end.

use serde_json::{json, Value};

use crate::catalog_drilldown::{array, call, ids, ok, router, text};
use crate::validation_message_rules::refused;

// ---------------------------------------------------------------------------
// Rule 1: the not-found message names the modelled resource.
// ---------------------------------------------------------------------------

/// Every operation that looks a resource up by id, with the id member it looks up and the
/// resource name its `ResourceNotFoundException` has to use.
///
/// Source: the id members and the shapes they target —
/// `smithy_query(query="operation",operation="GetListing,GetProduct,ListFulfillmentOptions,
/// GetOffer,GetOfferTerms,GetOfferSet")`: `listingId` -> `ListingId`, `productId` ->
/// `ProductId`, `offerId` -> `OfferId`, `offerSetId` -> `OfferSetId`. The resource is that
/// shape family's name without the `Id`, which is also how the model names the operation
/// that reads it (`GetOfferSet`).
const NOT_FOUND_CASES: &[(&str, &str, &str)] = &[
    ("getListing", "listingId", "Listing"),
    ("getProduct", "productId", "Product"),
    ("listFulfillmentOptions", "productId", "Product"),
    ("getOffer", "offerId", "Offer"),
    ("getOfferTerms", "offerId", "Offer"),
    ("getOfferSet", "offerSetId", "OfferSet"),
];

/// Well-formed ids no catalog record carries: all inside the `^[\w\-]+$` every id member
/// of this service declares, differing in length, shape and whether they look like one of
/// this catalog's own slugs.
const UNKNOWN_IDS: &[&str] = &[
    "does-not-exist",
    "unknown",
    "o-0001-analytics-annual-x",
    "x-00000000000000000000000000000000",
    "a",
];

#[tokio::test]
async fn a_not_found_message_names_the_modelled_resource() {
    let router = router();
    for (operation, member, resource) in NOT_FOUND_CASES {
        for unknown in UNKNOWN_IDS {
            let case = format!("{operation} {member}={unknown}");
            let (status, body) = call(&router, operation, &json!({ *member: unknown })).await;
            assert_eq!(
                status, 404,
                "case {case}: expected HTTP 404, got {status} with body {body}"
            );
            assert_eq!(
                text(&body, "__type"),
                "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {body}"
            );
            let message = text(&body, "message");
            assert_eq!(
                message,
                format!("{resource} not found"),
                "case {case}: expected the message '{resource} not found', got {message:?}"
            );
            assert!(
                !message.contains(*unknown) && !message.contains(*member),
                "case {case}: the message must carry no part of the request, got {message:?}"
            );
        }
    }
}

/// The positive control for rule 1: each of those operations resolves an id the catalog
/// DOES hold, so the refusals above are about the id and not the operation. The ids are
/// discovered from the catalog, and `GetOfferSet`'s record reads back under the id it
/// reports.
#[tokio::test]
async fn the_operations_that_refuse_an_unknown_id_resolve_a_known_one() {
    let router = router();

    let listings = ok(&router, "searchListings", &json!({ "maxResults": 100 })).await;
    let listing_id = ids(&array(&listings, "listingSummaries"), "listingId")
        .first()
        .cloned()
        .expect("a seeded listing");
    let listing = ok(&router, "getListing", &json!({ "listingId": &listing_id })).await;
    let paired = array(&listing, "associatedEntities")
        .into_iter()
        .find(|entity| entity.get("product").is_some() && entity.get("offer").is_some())
        .expect("a listing association carrying both a product and an offer");
    let product_id = text(&paired["product"], "productId");
    let offer_id = text(&paired["offer"], "offerId");

    let offer_sets = ok(
        &router,
        "listPurchaseOptions",
        &json!({
            "maxResults": 100,
            "filters": [{
                "filterType": "PURCHASE_OPTION_TYPE",
                "filterValues": ["OFFERSET"],
            }],
        }),
    )
    .await;
    let offer_set_id = ids(&array(&offer_sets, "purchaseOptions"), "purchaseOptionId")
        .first()
        .cloned()
        .expect("a seeded offer set");

    let known: &[(&str, &str, &String)] = &[
        ("getListing", "listingId", &listing_id),
        ("getProduct", "productId", &product_id),
        ("listFulfillmentOptions", "productId", &product_id),
        ("getOffer", "offerId", &offer_id),
        ("getOfferTerms", "offerId", &offer_id),
        ("getOfferSet", "offerSetId", &offer_set_id),
    ];
    for (operation, member, id) in known {
        let body = ok(&router, operation, &json!({ *member: id })).await;
        assert!(
            body.is_object(),
            "case {operation} {member}={id}: the known id must resolve, got {body}"
        );
    }
    assert_eq!(
        text(
            &ok(
                &router,
                "getOfferSet",
                &json!({ "offerSetId": &offer_set_id })
            )
            .await,
            "offerSetId"
        ),
        offer_set_id,
        "case getOfferSet offerSetId={offer_set_id}: the record must read back under the id \
         it reports"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the enum clause lists the value set in this service's order.
// ---------------------------------------------------------------------------

/// `SearchFilterType`'s value set as this service lists it in an enum clause — one fixed
/// order, which is NOT the order the model declares the values in
/// (`smithy_query(query="shape",shape="SearchFilterType")` declares
/// `MIN_AVERAGE_CUSTOMER_RATING` before `NUMBER_OF_PRODUCTS`): the nine values the recorded
/// refusal listed, in its order, then the six the model has gained since, in the model's.
const SEARCH_FILTER_TYPE_SET: &str = "[MAX_AVERAGE_CUSTOMER_RATING, FULFILLMENT_OPTION_TYPE, \
     MIN_AVERAGE_CUSTOMER_RATING, PRICING_MODEL, NUMBER_OF_PRODUCTS, DEPLOYED_ON_AWS, \
     PUBLISHER, PRICING_UNIT, CATEGORY, CERTIFICATES, PRODUCT_ID, SELLER_ENGAGEMENT, \
     SPECIALIZATION, SPECIALIZATION_CATEGORY, TRUST_CENTER]";

/// A request whose `filterType` falls outside `SearchFilterType`, with the member path the
/// clause has to name. Rows differ in the operation (both that carry `filters`), in where
/// in the list the bad element sits, and in the value that was rejected — none of which may
/// change the value set the clause lists.
fn enum_clause_cases() -> Vec<(&'static str, &'static str, Value, &'static str)> {
    vec![
        (
            "SearchListings, the only filter",
            "searchListings",
            json!({
                "maxResults": 5,
                "filters": [{ "filterType": "NOT_A_REAL_FILTER_TYPE", "filterValues": ["x"] }],
            }),
            "filters.1.member.filterType",
        ),
        (
            "SearchListings, the second of two filters",
            "searchListings",
            json!({
                "maxResults": 5,
                "filters": [
                    { "filterType": "CATEGORY", "filterValues": ["security"] },
                    { "filterType": "NOT_A_REAL_FILTER_TYPE", "filterValues": ["x"] },
                ],
            }),
            "filters.2.member.filterType",
        ),
        (
            "SearchFacets, the same member on the other operation",
            "searchFacets",
            json!({
                "filters": [{ "filterType": "min_average_customer_rating", "filterValues": ["1"] }],
            }),
            "filters.1.member.filterType",
        ),
        (
            "SearchFacets, a value of the other filter enum",
            "searchFacets",
            json!({
                "filters": [{ "filterType": "PURCHASE_OPTION_TYPE", "filterValues": ["OFFER"] }],
            }),
            "filters.1.member.filterType",
        ),
    ]
}

#[tokio::test]
async fn an_enum_clause_lists_the_value_set_in_this_services_order() {
    let router = router();
    for (case, operation, input, path) in enum_clause_cases() {
        let message = refused(&router, case, operation, &input).await;
        let rejected = input["filters"]
            .as_array()
            .and_then(|filters| filters.last())
            .map(|filter| text(filter, "filterType"))
            .unwrap_or_default();
        let expected = format!(
            "1 validation error detected: Value '{rejected}' at '{path}' failed to satisfy \
             constraint: Member must satisfy enum value set: {SEARCH_FILTER_TYPE_SET}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// The order is a property of the SET, not of the renderer: the other filter enum this
/// service declares keeps every value the model gives it. Pinned by membership and count,
/// since no recorded refusal fixes an order for this one.
///
/// Source: `smithy_query(query="shape",shape="PurchaseOptionFilterType")`.
const PURCHASE_OPTION_FILTER_TYPES: &[&str] = &[
    "PRODUCT_ID",
    "SELLER_OF_RECORD_PROFILE_ID",
    "PURCHASE_OPTION_TYPE",
    "VISIBILITY_SCOPE",
    "AVAILABILITY_STATUS",
];

#[tokio::test]
async fn an_enum_with_no_recorded_order_still_lists_every_modelled_value() {
    let router = router();
    let case = "ListPurchaseOptions filterType outside PurchaseOptionFilterType";
    let message = refused(
        &router,
        case,
        "listPurchaseOptions",
        &json!({
            "filters": [{ "filterType": "NOT_A_REAL_FILTER_TYPE", "filterValues": ["x"] }],
        }),
    )
    .await;
    let listed: Vec<&str> = message
        .split_once('[')
        .and_then(|(_, rest)| rest.strip_suffix(']'))
        .unwrap_or_default()
        .split(", ")
        .collect();
    assert_eq!(
        listed.len(),
        PURCHASE_OPTION_FILTER_TYPES.len(),
        "case {case:?}: expected {} admissible values, got {listed:?}",
        PURCHASE_OPTION_FILTER_TYPES.len()
    );
    for value in PURCHASE_OPTION_FILTER_TYPES {
        assert!(
            listed.contains(value),
            "case {case:?}: expected {value:?} among the admissible values, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a newline in `searchText` is refused by this service's own sentence.
// ---------------------------------------------------------------------------

/// The sentence this service refuses a `searchText` carrying a newline with: fixed text
/// naming the member, and nothing else. Shared with `tests/all/whole_value_patterns.rs`,
/// which pins the same refusal as the whole-value reading of the member's pattern.
pub(crate) const NEWLINE_REFUSAL: &str = "searchText must not contain newlines";

/// Values carrying a newline, differing in where it sits, how many there are, what else
/// the value holds, and how long it is — the whole class the member's modelled class
/// `[^\n]*` excludes (`smithy_query(query="shape",shape="SearchText")`).
fn newline_values() -> Vec<(&'static str, String)> {
    vec![
        ("a newline between two words", "line one\nline two".to_string()),
        ("a leading newline", "\nanalytics".to_string()),
        ("a trailing newline", "analytics\n".to_string()),
        ("nothing but a newline", "\n".to_string()),
        ("two newlines", "data\nanalytics\nfirewall".to_string()),
        (
            "a newline among punctuation and markup",
            "data & <analytics>\n(firewall)".to_string(),
        ),
        (
            "a carriage return beside the newline",
            "data\r\nanalytics".to_string(),
        ),
        (
            "a newline after non-ASCII text",
            "données\nanalytiques".to_string(),
        ),
        (
            "a newline at the member's maximum length",
            format!("{}\n", "x".repeat(511)),
        ),
        ("a newline among whitespace only", " \t\n\t ".to_string()),
    ]
}

/// The two operations that carry a `searchText`, and the request each one takes
/// (`SearchListings` is paged by `maxResults`, `SearchFacets` models none).
fn search_input(operation: &str, search_text: &str) -> Value {
    let mut input = json!({ "searchText": search_text });
    if operation == "searchListings" {
        input["maxResults"] = json!(100);
    }
    input
}

#[tokio::test]
async fn a_newline_in_search_text_is_refused_by_this_services_own_sentence() {
    let router = router();
    for operation in ["searchFacets", "searchListings"] {
        for (row, value) in newline_values() {
            let case = format!("{operation} searchText with {row}");
            let message = refused(&router, &case, operation, &search_input(operation, &value)).await;
            assert_eq!(
                message, NEWLINE_REFUSAL,
                "case {case:?}: expected {NEWLINE_REFUSAL:?}, got {message:?}"
            );
            // What the sentence must NOT carry: the validation-report envelope, the
            // expression the member is constrained by, or the submitted value.
            for leaked in ["validation error", "pattern", "constraint", "[^"] {
                assert!(
                    !message.contains(leaked),
                    "case {case:?}: the message must not carry {leaked:?}, got {message:?}"
                );
            }
            let submitted = value.trim();
            if !submitted.is_empty() {
                assert!(
                    !message.contains(submitted),
                    "case {case:?}: the message must not echo the submitted value \
                     {submitted:?}, got {message:?}"
                );
            }
        }
    }
}

/// The newline is the only thing this sentence refuses: a value the member's class admits
/// is still answered from the catalog, and a value breaking the member's `@length` is
/// still reported as the constraint violation it is, through the standard report.
#[tokio::test]
async fn the_newline_sentence_replaces_no_other_refusal() {
    let router = router();

    // Admitted: every other control character the class allows, plus a plain query.
    for (row, value) in [
        ("a carriage return", "data\ranalytics".to_string()),
        ("a tab", "data\tanalytics".to_string()),
        ("a vertical tab", "data\u{0b}analytics".to_string()),
        ("a plain term", "analytics".to_string()),
        ("the maximum length", "x".repeat(512)),
    ] {
        let case = format!("searchFacets searchText with {row}");
        let (status, body) = call(&router, "searchFacets", &search_input("searchFacets", &value)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert!(
            body.get("listingFacets").is_some(),
            "case {case:?}: expected the facets counted over the catalog, got {body}"
        );
    }

    // Refused by a constraint the framework's walk reports: the standard report stands.
    for (row, value, clause) in [
        (
            "an empty searchText, below its minimum length",
            String::new(),
            "Member must have length greater than or equal to 1",
        ),
        (
            "a searchText one character above its maximum length",
            "x".repeat(513),
            "Member must have length less than or equal to 512",
        ),
    ] {
        let case = format!("searchListings searchText with {row}");
        let message = refused(
            &router,
            &case,
            "searchListings",
            &search_input("searchListings", &value),
        )
        .await;
        assert!(
            message.starts_with("1 validation error detected: ") && message.ends_with(clause),
            "case {case:?}: expected the standard report ending in {clause:?}, got {message:?}"
        );
    }
}
