// SPDX-License-Identifier: CC0-1.0

//! Searching the catalog: what `SearchListings` and `SearchFacets` share.
//!
//! The keyword terms a `searchText` is matched by, the refusals a `filters` list draws that
//! the model's own `SearchFilterType` enum cannot state, and the predicate one listing is
//! held against — one copy, so the two search operations can never disagree about which
//! listings a query selects.

use super::*;

/// The terms a `searchText` is matched by: its whitespace-separated words, each lowercased
/// and stripped of the punctuation around it, de-duplicated and in the order the request
/// wrote them.
///
/// `SearchListings` is "search by keyword … sort results by relevance"
/// (`smithy_query(query="doc",shape="SearchListings")`), so its `searchText` is a query of
/// KEYWORDS rather than one literal string to be found: a two-word query matches a listing
/// whose content carries those words apart, and ranks one carrying both above one carrying
/// either. Splitting on whitespace only is what keeps a hyphenated or dotted identifier-like
/// word one term rather than silently widening it to its parts.
pub fn search_terms(search_text: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in search_text.split_whitespace() {
        let term = word
            .trim_matches(|character: char| !character.is_alphanumeric())
            .to_lowercase();
        if !term.is_empty() && !terms.contains(&term) {
            terms.push(term);
        }
    }
    terms
}

/// Refuse a `filters` list the model's `SearchFilterType` enum cannot reject on its own: a
/// filter type REPEATED across entries, an unsupported filter type, a range filter carrying
/// more than one value or a rating outside the documented 0.0–5.0 range, or a
/// `NUMBER_OF_PRODUCTS` value that is not a count.
///
/// Checked up front rather than per listing, so the refusal is a property of the REQUEST:
/// the same malformed filter is refused whether or not any listing reaches the predicate.
pub fn validate_search_filters(filters: &[SearchFilter]) -> Result<(), String> {
    reject_duplicate_filter_types(filters.iter().map(|filter| filter.filter_type.as_str()))?;
    for filter in filters {
        let values = &filter.filter_values;
        match filter.filter_type.as_str() {
            // Term filters take identifiers and enum values; the values themselves name
            // catalog records, and one no record carries simply matches nothing.
            //
            // `PRODUCT_ID` ("One or more product identifiers"), `SELLER_ENGAGEMENT`
            // ("One or more `SellerEngagementType` values"), `CERTIFICATES` ("One or more
            // certification identifiers"), `SPECIALIZATION` and
            // `SPECIALIZATION_CATEGORY` ("One or more specialization [category]
            // identifiers") and `TRUST_CENTER` ("The single value `HAS_TRUST_CENTER`") are
            // term filters too, so they are taken the same way
            // (`SearchFilter$filterValues`).
            "CATEGORY" | "PUBLISHER" | "FULFILLMENT_OPTION_TYPE" | "PRICING_MODEL"
            | "PRICING_UNIT" | "DEPLOYED_ON_AWS" | "PRODUCT_ID" | "SELLER_ENGAGEMENT"
            | "CERTIFICATES" | "SPECIALIZATION" | "SPECIALIZATION_CATEGORY"
            | "TRUST_CENTER" => {}
            "NUMBER_OF_PRODUCTS" => {
                for value in values {
                    product_count(value)?;
                }
            }
            "MIN_AVERAGE_CUSTOMER_RATING" | "MAX_AVERAGE_CUSTOMER_RATING" => {
                rating_bound(&filter.filter_type, values)?;
            }
            other => {
                return Err(format!("filters: '{other}' is not a supported filterType"));
            }
        }
    }
    Ok(())
}

/// Refuse a `filters` list that names one `filterType` more than once, naming every type it
/// repeats.
///
/// Why one entry per type is the rule: a filter entry of this service carries the values to
/// filter that type by, and both filter shapes document those values as OR-combined —
/// `SearchFilter$filterValues` "Term filters accept multiple values (OR logic)" and
/// `PurchaseOptionFilter$filterValues` "OR logic combines multiple values within the same
/// filter". So the OR of several values of ONE filter type belongs inside that entry's
/// `filterValues`, which leaves a second entry for the same type with nothing to express —
/// and the predicates below AND the entries, so such a list selects only what satisfies
/// both. The rule is independent of a list's own `@length` bound (1..30 for a search,
/// 1..10 for purchase options): two entries are well inside either.
///
/// The message names the offending types from the REQUEST, bracketed, and in the order the
/// request first repeated them, so a list repeating two types names both. Taken by filter
/// TYPE rather than by filter shape, so the one rule serves every `filters` member of this
/// service.
pub fn reject_duplicate_filter_types<'a>(
    filter_types: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let mut seen: Vec<&str> = Vec::new();
    let mut repeated: Vec<&str> = Vec::new();
    for filter_type in filter_types {
        if seen.contains(&filter_type) {
            if !repeated.contains(&filter_type) {
                repeated.push(filter_type);
            }
        } else {
            seen.push(filter_type);
        }
    }
    if repeated.is_empty() {
        return Ok(());
    }
    Err(format!("Duplicate filter types: [{}]", repeated.join(", ")))
}

/// Whether a listing satisfies every search filter (AND across filters, OR within one).
///
/// Source: `SearchListingsInput$filters` / `SearchFacetsInput$filters` — "Multiple filters
/// are combined with AND logic. Multiple values within the same filter are combined with OR
/// logic."
///
/// `Err` is the refusal message for a filter value the model's `SearchFilterType` enum
/// cannot reject on its own — a range filter with more than one value, or a rating outside
/// the documented 0.0–5.0 range.
pub fn matches_every_search_filter(
    state: &ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState,
    listing: &Listing,
    filters: &[SearchFilter],
) -> Result<bool, String> {
    for filter in filters {
        let values = &filter.filter_values;
        let matched = match filter.filter_type.as_str() {
            "CATEGORY" => listing
                .categories
                .iter()
                .any(|category| values.contains(&category.category_id)),
            "PUBLISHER" => values.contains(&listing.publisher.seller_profile_id),
            "FULFILLMENT_OPTION_TYPE" => state
                .listing_fulfillment_option_summaries(listing)
                .iter()
                .any(|option| values.contains(&option.fulfillment_option_type)),
            "PRICING_MODEL" => state
                .listing_pricing_models(listing)
                .iter()
                .any(|model| values.contains(&model.pricing_model_type)),
            "PRICING_UNIT" => state
                .listing_pricing_units(listing)
                .iter()
                .any(|unit| values.contains(&unit.pricing_unit_type)),
            "DEPLOYED_ON_AWS" => listing.product_ids.iter().any(|product_id| {
                state
                    .products
                    .get(product_id)
                    .is_some_and(|product| {
                        values.iter().any(|value| value == product.deployed_on_aws())
                    })
            }),
            "NUMBER_OF_PRODUCTS" => {
                let products = listing.product_ids.len();
                let mut matched = false;
                for value in values {
                    matched |= product_count(value)? == products;
                }
                matched
            }
            // "One or more product identifiers. Matches listings that contain those
            // products" — the products the listing's own record names.
            "PRODUCT_ID" => listing
                .product_ids
                .iter()
                .any(|product_id| values.contains(product_id)),
            // "One or more `SellerEngagementType` values … Matches listings that offer those
            // ways to engage with the seller" — the engagements the listing's own record
            // offers, which is the list `GetListing` reports.
            "SELLER_ENGAGEMENT" => listing
                .seller_engagements
                .iter()
                .any(|engagement| values.contains(&engagement.engagement_type)),
            // The three term filters over a listing's ATTRIBUTES, each matched on exactly
            // the values the attribute read reports and the facet counts
            // (`crate::state::listing_attributes`), so a value taken from a facet selects
            // the listings that value was counted over.
            "CERTIFICATES" | "SPECIALIZATION" | "SPECIALIZATION_CATEGORY" | "TRUST_CENTER" => {
                state
                    .listing_attribute_facet_values(listing, &filter.filter_type)
                    .iter()
                    .any(|(value, _)| values.contains(value))
            }
            // Range filters: "accept a single value between 0.0 and 5.0"
            // (SearchFilter$filterValues documentation).
            "MIN_AVERAGE_CUSTOMER_RATING" => {
                let bound = rating_bound(&filter.filter_type, values)?;
                listing.average_customer_rating() >= bound
            }
            "MAX_AVERAGE_CUSTOMER_RATING" => {
                let bound = rating_bound(&filter.filter_type, values)?;
                listing.average_customer_rating() <= bound
            }
            other => {
                return Err(format!("filters: '{other}' is not a supported filterType"));
            }
        };
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The product count a `NUMBER_OF_PRODUCTS` filter value names, refusing a value that is
/// not a non-negative integer.
fn product_count(value: &str) -> Result<usize, String> {
    value.parse().map_err(|_| {
        format!("filters: NUMBER_OF_PRODUCTS expects a non-negative integer, got '{value}'")
    })
}

/// The single rating a range filter carries, refusing a value list or a rating outside the
/// documented 0.0–5.0 range.
///
/// Both refusals are built from the request and follow AWS's recorded template for a range
/// filter: they name the offending `filterType`, and the out-of-range one also carries the
/// offending value bare after `"got: "`. A value that is not a number at all is not a
/// rating between 0.0 and 5.0 either, so it is refused by the same message.
fn rating_bound(filter_type: &str, values: &[String]) -> Result<f64, String> {
    let [value] = values else {
        return Err(format!(
            "Range filter {filter_type} requires exactly one value"
        ));
    };
    let rating: f64 = value.parse().unwrap_or(f64::NAN);
    if !(0.0..=5.0).contains(&rating) {
        return Err(format!(
            "Range filter {filter_type} value must be between 0.0 and 5.0, got: {value}"
        ));
    }
    Ok(rating)
}
