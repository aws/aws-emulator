// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a constraint violation: the `ValidationException` report the
//! framework's input walk hands to `EmulatedService::override_refusal`.
//!
//! Two rules of how the report is BUILT are pinned here, over a table of inputs rather
//! than over one recorded request:
//!
//! 1. a `@pattern` clause reports the modeled expression with its `^` / `$` anchors
//!    stripped and everything between them verbatim — for EVERY member carrying a
//!    `@pattern`, at the top level and nested inside a list;
//! 2. the echoed value is entity-escaped TWICE, so a caller's `&`, `<` or `>` reaches
//!    the client as `&amp;amp;`, `&amp;lt;`, `&amp;gt;`.
//!
//! A constraint broken by a list ELEMENT is reported at the LIST member, echoing the whole
//! list and nesting the element's requirement inside `Member must satisfy constraint:
//! [ … ]` — the form `tests/all/stated_refusal_wording.rs` owns; the rows below that carry a
//! nested list are stated in that form.
//!
//! The suites around them pin what the two rules must NOT disturb: the error code
//! (`ValidationException`), the HTTP status (400), the `N validation error(s) detected:`
//! envelope, the member path, the `@length` / `@range` / enum clauses, and the bounds at
//! which a request is accepted rather than refused.
//!
//! Every request goes through the `MultiServiceRouter`, so the protocol, the routing
//! table and the framework's input walk are all exercised end to end.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The route prefix, the SigV4 header and the one-service router this crate's suites
// share, declared once in `catalog_drilldown` and reused here rather than copied.
use crate::catalog_drilldown::{router, AUTHORIZATION, PREFIX};

/// The error code and HTTP status every constraint refusal keeps, whatever the clause
/// says: `ValidationException` (Smithy `errorKind` client, `@httpError 400`) is declared
/// by every operation of this service.
pub(crate) const VALIDATION_EXCEPTION: &str = "ValidationException";
pub(crate) const VALIDATION_STATUS: u16 = 400;

/// Call one operation and return its HTTP status, its `x-amzn-errortype` header and its
/// JSON body.
///
/// `catalog_drilldown::call` answers status and body; this suite asserts on the
/// `x-amzn-errortype` header too (restJson1 resolves the error SHAPE from it), which is
/// why it dispatches the request itself.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    input: &Value,
) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("{PREFIX}/{operation}"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(input.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, errortype, body)
}

/// Call one operation expecting a refused request, and return the `message` the report
/// carries. Checks the parts of the answer no clause rule may change: HTTP 400, the
/// `ValidationException` code in the body's `__type` and in `x-amzn-errortype`.
pub(crate) async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    input: &Value,
) -> String {
    let (status, errortype, body) = call(router, operation, input).await;
    assert_eq!(
        status, VALIDATION_STATUS,
        "case {case:?}: expected HTTP {VALIDATION_STATUS}, got {status} with body {body}"
    );
    assert_eq!(
        body.get("__type").and_then(Value::as_str),
        Some(VALIDATION_EXCEPTION),
        "case {case:?}: expected __type {VALIDATION_EXCEPTION}, got {body}"
    );
    assert_eq!(
        errortype, VALIDATION_EXCEPTION,
        "case {case:?}: expected x-amzn-errortype {VALIDATION_EXCEPTION}, got {errortype:?}"
    );
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Call one operation expecting it to be accepted.
async fn accepted(router: &MultiServiceRouter, case: &str, operation: &str, input: &Value) {
    let (status, _, body) = call(router, operation, input).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
}

/// The one-violation envelope AWS reports a single constraint failure in.
fn one_violation(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

// ---------------------------------------------------------------------------
// Rule 1: a `@pattern` clause reports the modeled expression UNANCHORED.
// ---------------------------------------------------------------------------

/// Every `@pattern`-carrying input member of this service, with the expression the
/// report must quote for it: the model's `@pattern` without the leading `^` and the
/// trailing `$`, and byte-identical between them.
///
/// Source: `smithy_query(query="operation", operation="GetListing,GetProduct,GetOffer,
/// SearchListings,ListPurchaseOptions")` and `smithy_query(query="shape",
/// shape="SearchFilterValueList,PurchaseOptionFilterValueList")` on
/// `services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json`:
/// `ListingId` / `ProductId` / `OfferId` / `PurchaseOptionFilterValue` are
/// `^[\w\-]+$`, `Locale` is `^[^;,]+(\s*,\s*[^;,]+){0,1}$`, `NextToken` is
/// `^[a-zA-Z0-9+/=]+$`, `SearchFilterValue` is `^[\w\-\.]+$`.
///
/// Rows beyond the one the report was filed against (`listingId`) are here on purpose:
/// the rule is about the renderer, so it holds for a member the filing never mentioned
/// and for one nested inside a list.
struct PatternCase {
    case: &'static str,
    operation: &'static str,
    /// The violating value, as the report must echo it.
    value: &'static str,
    /// The dotted member path the report must name.
    path: &'static str,
    /// The expression the report must quote.
    expression: &'static str,
    /// Whether the member is a LIST whose element broke the pattern, which is reported at
    /// the list with the element's requirement nested.
    nested: bool,
    /// The request body carrying `value` at `path`.
    input: fn(&str) -> Value,
}

fn pattern_cases() -> Vec<PatternCase> {
    vec![
        PatternCase {
            case: "GetListing listingId outside ^[\\w\\-]+$",
            operation: "getListing",
            value: "not a valid id!",
            path: "listingId",
            expression: r"[\w\-]+",
            nested: false,
            input: |value| json!({ "listingId": value }),
        },
        PatternCase {
            case: "GetProduct productId outside ^[\\w\\-]+$",
            operation: "getProduct",
            value: "product id with spaces",
            path: "productId",
            expression: r"[\w\-]+",
            nested: false,
            input: |value| json!({ "productId": value }),
        },
        PatternCase {
            case: "GetOffer offerId outside ^[\\w\\-]+$",
            operation: "getOffer",
            value: "offer/id/with/slashes",
            path: "offerId",
            expression: r"[\w\-]+",
            nested: false,
            input: |value| json!({ "offerId": value }),
        },
        PatternCase {
            case: "GetListing locale outside its two-tag pattern",
            operation: "getListing",
            value: "en,fr,de",
            path: "locale",
            expression: r"[^;,]+(\s*,\s*[^;,]+){0,1}",
            nested: false,
            input: |value| json!({ "listingId": "listing-1", "locale": value }),
        },
        PatternCase {
            case: "SearchListings nextToken outside ^[a-zA-Z0-9+/=]+$",
            operation: "searchListings",
            value: "not a token!",
            path: "nextToken",
            expression: "[a-zA-Z0-9+/=]+",
            nested: false,
            input: |value| json!({ "maxResults": 5, "nextToken": value }),
        },
        PatternCase {
            case: "SearchListings filter value outside ^[\\w\\-\\.]+$, nested in a list",
            operation: "searchListings",
            value: "category id!",
            path: "filters.1.member.filterValues",
            expression: r"[\w\-\.]+",
            nested: true,
            input: |value| {
                json!({
                    "maxResults": 5,
                    "filters": [{ "filterType": "CATEGORY", "filterValues": [value] }],
                })
            },
        },
        PatternCase {
            case: "ListPurchaseOptions filter value outside ^[\\w\\-]+$, nested in a list",
            operation: "listPurchaseOptions",
            value: "product id!",
            path: "filters.1.member.filterValues",
            expression: r"[\w\-]+",
            nested: true,
            input: |value| {
                json!({
                    "filters": [{ "filterType": "PRODUCT_ID", "filterValues": [value] }],
                })
            },
        },
    ]
}

#[tokio::test]
async fn pattern_clause_quotes_the_modeled_expression_without_its_anchors() {
    let router = router();
    for PatternCase {
        case,
        operation,
        value,
        path,
        expression,
        nested,
        input,
    } in pattern_cases()
    {
        let message = refused(&router, case, operation, &input(value)).await;
        // A clause about a list's ELEMENT is stated at the list, echoing the whole list and
        // nesting the element's requirement.
        let (echoed, requirement) = if nested {
            (
                format!("[{value}]"),
                format!(
                    "Member must satisfy constraint: \
                     [Member must satisfy regular expression pattern: {expression}]"
                ),
            )
        } else {
            (
                value.to_string(),
                format!("Member must satisfy regular expression pattern: {expression}"),
            )
        };
        let expected = one_violation(&format!(
            "Value '{echoed}' at '{path}' failed to satisfy constraint: {requirement}"
        ));
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // The anchors are what the rule strips: the quoted expression may not OPEN with
        // `^` nor CLOSE with `$` (a `^` inside a character class, as `Locale` has, is
        // part of the expression and stays).
        assert!(
            !message.contains("pattern: ^") && !message.ends_with('$'),
            "case {case:?}: the quoted expression must carry no ^ or $ anchor, \
             got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the echoed value is entity-escaped TWICE.
// ---------------------------------------------------------------------------

/// A rejected value and the text the report must echo for it. Every row violates
/// `ListingId`'s `^[\w\-]+$` (no markup character is in `\w` or `-`), so the only
/// difference between rows is what entity escaping has to do with the value: nothing,
/// one `&`, one `<`, one `>`, several, and all three at once.
const ESCAPE_CASES: &[(&str, &str, &str)] = &[
    (
        "no markup character in the value",
        "plain invalid id!",
        "plain invalid id!",
    ),
    ("one ampersand", "a&b", "a&amp;amp;b"),
    ("ampersand alone", "&", "&amp;amp;"),
    ("one less-than", "a<b", "a&amp;lt;b"),
    ("one greater-than", "a>b", "a&amp;gt;b"),
    ("two ampersands", "x&&y", "x&amp;amp;&amp;amp;y"),
    (
        "all three markup characters",
        "<&>",
        "&amp;lt;&amp;amp;&amp;gt;",
    ),
    (
        "an already-escaped-looking value escapes again",
        "a&amp;b",
        "a&amp;amp;amp;b",
    ),
    (
        "the ampersand in prose",
        "not a valid id! (has spaces & punctuation)",
        "not a valid id! (has spaces &amp;amp; punctuation)",
    ),
];

#[tokio::test]
async fn echoed_value_is_entity_escaped_twice() {
    let router = router();
    for (case, value, echoed) in ESCAPE_CASES {
        let message = refused(&router, case, "getListing", &json!({ "listingId": value })).await;
        let expected = one_violation(&format!(
            "Value '{echoed}' at 'listingId' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: {}",
            r"[\w\-]+"
        ));
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// The escaping applies wherever `Value '<value>'` is produced, not only to a
/// `@pattern` clause: a `@length` violation echoes its value through the same step.
#[tokio::test]
async fn escaping_applies_to_every_clause_that_echoes_a_value() {
    let router = router();
    // `SearchFilterValue` is @length 1..128, so a 129-character value carrying an
    // ampersand fails on length while the value still has to be escaped.
    let value = format!("{}&", "v".repeat(128));
    let case = "a length violation echoing an ampersand";
    let message = refused(
        &router,
        case,
        "searchListings",
        &json!({
            "maxResults": 5,
            "filters": [{ "filterType": "CATEGORY", "filterValues": [value] }],
        }),
    )
    .await;
    // The value also falls outside the member's `@pattern` (an ampersand is in neither
    // `\w` nor `-`), so the report carries two clauses; what this pins is that the
    // LENGTH clause echoes the value through the same escaping step. The clause is stated
    // at the LIST member, echoing the whole list the caller sent.
    let expected = format!(
        "Value '[{}&amp;amp;]' at 'filters.1.member.filterValues' \
         failed to satisfy constraint: Member must satisfy constraint: \
         [Member must have length less than or equal to 128]",
        "v".repeat(128)
    );
    assert!(
        message.contains(&expected),
        "case {case:?}: expected the clause {expected:?}, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// What the two rules must not disturb: the other clauses and the envelope.
// ---------------------------------------------------------------------------

/// A `@length` or `@range` clause, at each published bound and one step past it.
///
/// Bounds from `smithy_query`: `MaxResults` @range 1..100 (default 25), `SearchText`
/// @length 1..512, `NextToken` @length 0..8192.
#[tokio::test]
async fn length_and_range_clauses_report_the_modeled_bound() {
    let router = router();

    // Accepted AT each published bound.
    for (case, max_results) in [("maxResults at its minimum", 1), ("maxResults at its maximum", 100)]
    {
        accepted(
            &router,
            case,
            "searchListings",
            &json!({ "maxResults": max_results }),
        )
        .await;
    }
    accepted(
        &router,
        "searchText at its maximum length",
        "searchListings",
        &json!({ "maxResults": 1, "searchText": "x".repeat(512) }),
    )
    .await;
    accepted(
        &router,
        "searchText at its minimum length",
        "searchListings",
        &json!({ "maxResults": 1, "searchText": "x" }),
    )
    .await;

    // Refused one step PAST each bound, naming the bound the model declares.
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "maxResults one above its maximum",
            "searchListings",
            json!({ "maxResults": 101 }),
            "Value '101' at 'maxResults' failed to satisfy constraint: \
             Member must have value less than or equal to 100"
                .to_string(),
        ),
        (
            "maxResults one below its minimum",
            "searchListings",
            json!({ "maxResults": 0 }),
            "Value '0' at 'maxResults' failed to satisfy constraint: \
             Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            "maxResults one above its maximum on another operation",
            "listPurchaseOptions",
            json!({ "maxResults": 101 }),
            "Value '101' at 'maxResults' failed to satisfy constraint: \
             Member must have value less than or equal to 100"
                .to_string(),
        ),
        (
            "searchText one below its minimum length",
            "searchListings",
            json!({ "maxResults": 1, "searchText": "" }),
            "Value '' at 'searchText' failed to satisfy constraint: \
             Member must have length greater than or equal to 1"
                .to_string(),
        ),
        (
            "an empty filter list, below the list's minimum length",
            "listPurchaseOptions",
            json!({ "filters": [] }),
            "Value '0 items' at 'filters' failed to satisfy constraint: \
             Member must have length greater than or equal to 1"
                .to_string(),
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

/// A value longer than the echo the report carries still names the bound it broke, at
/// the member that broke it. The echo itself is capped by the framework's shared
/// constraint walk (`vendor/smithy-schema-server`, `MAX_ECHOED_VALUE`), which elides a
/// value over 256 bytes before any service-level renderer sees it, so what is pinned
/// here is the clause and the echoed PREFIX.
#[tokio::test]
async fn a_value_past_its_length_bound_names_the_bound_and_the_member() {
    let router = router();
    let case = "searchText one character above its maximum length";
    let value = "x".repeat(513);
    let message = refused(
        &router,
        case,
        "searchListings",
        &json!({ "maxResults": 1, "searchText": value }),
    )
    .await;
    assert!(
        message.ends_with(
            "at 'searchText' failed to satisfy constraint: \
             Member must have length less than or equal to 512"
        ),
        "case {case:?}: expected the 512 length clause at 'searchText', got {message:?}"
    );
    assert!(
        message.starts_with(&format!("1 validation error detected: Value '{}", "x".repeat(256))),
        "case {case:?}: expected the echo to open with the value that was sent, \
         got {message:?}"
    );
}

/// An enum clause reports the admissible value SET — every value the model declares,
/// and nothing else. The order inside the brackets is not asserted: a set has no order,
/// and neither the model nor AWS's documentation fixes one.
///
/// Source: `smithy_query(query="shape", shape="PurchaseOptionFilterType,
/// SearchFilterType")`.
const PURCHASE_OPTION_FILTER_TYPES: &[&str] = &[
    "PRODUCT_ID",
    "SELLER_OF_RECORD_PROFILE_ID",
    "PURCHASE_OPTION_TYPE",
    "VISIBILITY_SCOPE",
    "AVAILABILITY_STATUS",
];
const SEARCH_FILTER_TYPES: &[&str] = &[
    "MIN_AVERAGE_CUSTOMER_RATING",
    "MAX_AVERAGE_CUSTOMER_RATING",
    "CATEGORY",
    "PUBLISHER",
    "FULFILLMENT_OPTION_TYPE",
    "PRICING_MODEL",
    "PRICING_UNIT",
    "DEPLOYED_ON_AWS",
    "NUMBER_OF_PRODUCTS",
    "PRODUCT_ID",
    "SELLER_ENGAGEMENT",
    "CERTIFICATES",
    "SPECIALIZATION",
    "SPECIALIZATION_CATEGORY",
    "TRUST_CENTER",
];

#[tokio::test]
async fn enum_clause_reports_the_whole_modeled_value_set() {
    let router = router();
    let cases: Vec<(&str, &str, Value, &[&str])> = vec![
        (
            "ListPurchaseOptions filterType outside PurchaseOptionFilterType",
            "listPurchaseOptions",
            json!({
                "filters": [{
                    "filterType": "NOT_A_REAL_FILTER_TYPE",
                    "filterValues": ["x"],
                }],
            }),
            PURCHASE_OPTION_FILTER_TYPES,
        ),
        (
            "SearchListings filterType outside SearchFilterType",
            "searchListings",
            json!({
                "maxResults": 5,
                "filters": [{ "filterType": "NOT_A_REAL_FILTER_TYPE", "filterValues": ["x"] }],
            }),
            SEARCH_FILTER_TYPES,
        ),
    ];
    for (case, operation, input, values) in cases {
        let message = refused(&router, case, operation, &input).await;
        let opening = "1 validation error detected: Value 'NOT_A_REAL_FILTER_TYPE' at \
             'filters.1.member.filterType' failed to satisfy constraint: \
             Member must satisfy enum value set: [";
        assert!(
            message.starts_with(opening) && message.ends_with(']'),
            "case {case:?}: expected the enum clause at 'filters.1.member.filterType', \
             got {message:?}"
        );
        let listed: Vec<&str> = message
            .strip_prefix(opening)
            .and_then(|rest| rest.strip_suffix(']'))
            .unwrap_or_default()
            .split(", ")
            .collect();
        assert_eq!(
            listed.len(),
            values.len(),
            "case {case:?}: expected {} admissible values, got {listed:?}",
            values.len()
        );
        for value in values {
            assert!(
                listed.contains(value),
                "case {case:?}: expected {value:?} among the admissible values, \
                 got {listed:?}"
            );
        }
    }
}

/// An absent required member, and more than one violation in one request: the envelope
/// counts what it reports and joins the clauses with `; `.
#[tokio::test]
async fn the_envelope_counts_and_joins_the_clauses_it_reports() {
    let router = router();

    let case = "GetListing without its required listingId";
    let message = refused(&router, case, "getListing", &json!({})).await;
    assert_eq!(
        message,
        one_violation(
            "Value null at 'listingId' failed to satisfy constraint: Member must not be null"
        ),
        "case {case:?}: got {message:?}"
    );

    // An empty listingId breaks BOTH its @length minimum and its @pattern.
    let case = "GetListing with an empty listingId, breaking two constraints";
    let message = refused(&router, case, "getListing", &json!({ "listingId": "" })).await;
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {case:?}: expected the two-violation envelope, got {message:?}"
    );
    for clause in [
        "Value '' at 'listingId' failed to satisfy constraint: \
         Member must have length greater than or equal to 1",
        &format!(
            "Value '' at 'listingId' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: {}",
            r"[\w\-]+"
        ),
    ] {
        assert!(
            message.contains(clause),
            "case {case:?}: expected the clause {clause:?}, got {message:?}"
        );
    }
    assert_eq!(
        message.matches("; ").count(),
        1,
        "case {case:?}: expected the two clauses joined by '; ', got {message:?}"
    );
}

/// A request inside every constraint reaches the handler and is answered from state,
/// which is what makes the refusals above refusals rather than the only answer this
/// service has.
#[tokio::test]
async fn a_request_inside_every_constraint_is_answered_from_state() {
    let router = router();
    let (status, _, body) = call(&router, "searchListings", &json!({ "maxResults": 5 })).await;
    assert_eq!(
        status, 200,
        "case a valid SearchListings: expected HTTP 200, got {status} with body {body}"
    );
    let summaries = body
        .get("listingSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !summaries.is_empty(),
        "case a valid SearchListings: expected the seeded catalog, got {body}"
    );
    // A listing the search returned reads back through GetListing with the id it
    // reported, so the refusals above are the only reason an id is rejected.
    let listing_id = summaries[0]
        .get("listingId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let (status, _, listing) =
        call(&router, "getListing", &json!({ "listingId": listing_id })).await;
    assert_eq!(
        status, 200,
        "case GetListing on a searched id: expected HTTP 200, got {status} with body {listing}"
    );
    assert_eq!(
        listing.get("listingId").and_then(Value::as_str),
        Some(listing_id.as_str()),
        "case GetListing on a searched id: expected listingId {listing_id:?}, got {listing}"
    );
}
