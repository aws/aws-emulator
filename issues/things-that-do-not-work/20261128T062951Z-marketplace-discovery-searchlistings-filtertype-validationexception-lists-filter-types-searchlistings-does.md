Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-discovery-error-validation-bad-inputs
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-discovery-error-validation-bad-inputs
Issue-Shape-Id: com.amazonaws.marketplacediscovery#AWSMarketplaceDiscovery
Issue-Key: marketplace-discovery|SearchListings|4xx->4xx|$.message
Issue-Commit: ed3ed06301643834f5ccb593614cfd988c817634

# SearchListings filterType ValidationException lists filter types SearchListings does not accept

> _Note: this issue was identified against the code base at commit `ed3ed06301643834f5ccb593614cfd988c817634`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchListings

## Description
One cause.

Cause 1 of 1: the enum value set rendered in the SearchListings `filters[].filterType` ValidationException message is wrong — the emulator enumerates the complete `SearchFilterType` enum instead of the filter types the SearchListings search surface actually accepts.

AWS: recorded behavior (the message's accepted-value list is not stated by the model). A SearchListings request whose `filters` list carries an unmodelled `filterType` is refused with HTTP 400 ValidationException and the standard Smithy-constraint sentence: fixed text `"<N> validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must satisfy enum value set: [<accepted values>]"`. Its variable parts are the error count, the offending value taken from the request, the member path (here the list-member path `filters.<1-based index>.member.filterType`), and the accepted-value list. In the recording that list is the nine filter types the search surface accepts, in this order: MAX_AVERAGE_CUSTOMER_RATING, FULFILLMENT_OPTION_TYPE, MIN_AVERAGE_CUSTOMER_RATING, PRICING_MODEL, NUMBER_OF_PRODUCTS, DEPLOYED_ON_AWS, PUBLISHER, PRICING_UNIT, CATEGORY. Smithy (com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json): SearchListings POST /2026-02-05/searchListings, input member `filters` -> SearchFilterList (@length 1..30) of SearchFilter, whose `filterType` is required and targets the enum SearchFilterType; SearchListings models ValidationException with httpError 400. The model's SearchFilterType declares fifteen values — the nine above plus CERTIFICATES, PRODUCT_ID, SPECIALIZATION, SPECIALIZATION_CATEGORY, SELLER_ENGAGEMENT and TRUST_CENTER — so the enum shape is a superset of what the recorded refusal advertises for this operation. Hypothesis: the six extra values belong to other filter surfaces (and/or are not search-filterable), which is why AWS's SearchListings refusal omits them.

Emulator: same status (400) and same error class, and the fixed text, count, offending value and member path all match, but the accepted-value list is the entire enum: `[MAX_AVERAGE_CUSTOMER_RATING, FULFILLMENT_OPTION_TYPE, MIN_AVERAGE_CUSTOMER_RATING, PRICING_MODEL, NUMBER_OF_PRODUCTS, DEPLOYED_ON_AWS, PUBLISHER, PRICING_UNIT, CATEGORY, CERTIFICATES, PRODUCT_ID, SELLER_ENGAGEMENT, SPECIALIZATION, SPECIALIZATION_CATEGORY, TRUST_CENTER]` — the nine recorded values in AWS's order with six values appended that AWS does not advertise for this operation.

Why: the message is the only place AWS tells a caller which filter types this search accepts. A client (or an operator reading the refusal) that reads the list back and retries with one of the six surplus values is told by the emulator that the value is acceptable, while the real service refuses it with the same ValidationException — so the emulator teaches a request shape that fails against AWS. Tests and SDK/CLI wrappers that assert on the refusal text, or that derive their allowed-filter list from it, diverge on a stable (not per-run) part of the message.

Fix: SearchListings' input validation / its ValidationException message rendering for enum members — the accepted-value list for `filters[].filterType` must be the search-accepted subset in the recorded order, not the full SearchFilterType enum emitted straight from the model. The enum set belongs to the operation's validation layer, not to the shared Smithy-enum formatter.

Traces: marketplace-discovery-error-validation-bad-inputs#task#1 (SearchListings, verdict mismatch on $.message, expected 400 / actual 400).

## Evidence
Fixture marketplace-discovery-error-validation-bad-inputs, step task#1 (SearchListings, POST /2026-02-05/searchListings). Status matches (expected 400, actual 400); $.message diverges (value_mismatch).

Expected (recorded AWS): "1 validation error detected: Value 'NOT_A_REAL_FILTER_TYPE' at 'filters.1.member.filterType' failed to satisfy constraint: Member must satisfy enum value set: [MAX_AVERAGE_CUSTOMER_RATING, FULFILLMENT_OPTION_TYPE, MIN_AVERAGE_CUSTOMER_RATING, PRICING_MODEL, NUMBER_OF_PRODUCTS, DEPLOYED_ON_AWS, PUBLISHER, PRICING_UNIT, CATEGORY]"

Actual (emulator): "1 validation error detected: Value 'NOT_A_REAL_FILTER_TYPE' at 'filters.1.member.filterType' failed to satisfy constraint: Member must satisfy enum value set: [MAX_AVERAGE_CUSTOMER_RATING, FULFILLMENT_OPTION_TYPE, MIN_AVERAGE_CUSTOMER_RATING, PRICING_MODEL, NUMBER_OF_PRODUCTS, DEPLOYED_ON_AWS, PUBLISHER, PRICING_UNIT, CATEGORY, CERTIFICATES, PRODUCT_ID, SELLER_ENGAGEMENT, SPECIALIZATION, SPECIALIZATION_CATEGORY, TRUST_CENTER]"

Delta: six surplus accepted values appended (CERTIFICATES, PRODUCT_ID, SELLER_ENGAGEMENT, SPECIALIZATION, SPECIALIZATION_CATEGORY, TRUST_CENTER). Fixed text, error count, the offending value echoed from the request and the list-member path are all correct. Model cross-check: smithy_query shape SearchFilterType = 15 values (superset of the nine recorded); SearchFilter.filterType is required and targets that enum; SearchListings errors include ValidationException @httpError 400.

Not covered by any open marketplace-discovery issue: the five open ones (SearchListings result-page logo/review URLs and page-1 substitution, GetListing/GetProduct/ListFulfillmentOptions substituted catalog record, ListPurchaseOptions empty list, published fetch addresses/pagination token) all concern 200-response bodies and tokens, none the 400 ValidationException message for filterType.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture marketplace-discovery-error-validation-bad-inputs, step task#1.
2. Equivalently, by hand: POST /2026-02-05/searchListings to the marketplace-discovery endpoint with a body whose `filters` list has a second entry carrying filterType = "NOT_A_REAL_FILTER_TYPE" (e.g. {"searchText":"data","filters":[{"filterType":"CATEGORY","filterValues":["..."]},{"filterType":"NOT_A_REAL_FILTER_TYPE","filterValues":["x"]}]}).
3. Observe HTTP 400 ValidationException and read $.message: the bracketed accepted-value list contains fifteen values. AWS lists only the nine search-accepted filter types (MAX_AVERAGE_CUSTOMER_RATING, FULFILLMENT_OPTION_TYPE, MIN_AVERAGE_CUSTOMER_RATING, PRICING_MODEL, NUMBER_OF_PRODUCTS, DEPLOYED_ON_AWS, PUBLISHER, PRICING_UNIT, CATEGORY) in that order.
