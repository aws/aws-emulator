Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: d0b7c7f06678596bb74c7aa80db04e65e20adb2f

# A request that omits maxResults is answered as one unbounded page, ignoring the documented default of 100 that the crate's other paginated reads apply

_Filed: 2026-11-01T12:33:34Z_
_Operation: ListBillingViews | Class: pagination_

> _Note: this issue was identified against the code base at commit `d0b7c7f06678596bb74c7aa80db04e65e20adb2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListBillingViews` is `@paginated` and its `maxResults` member is documented with a default ("Default is 100"). When the request omits `maxResults` the handler takes the `_ => (matched, None)` branch: the whole matched set is serialized as a single page and `nextToken` is `None`, however many views the account holds (the store admits up to `MAX_BILLING_VIEWS = 256`). A client that pages correctly — call, follow `nextToken` — never sees a second page, and a client that sizes its buffers from the documented default receives more than 100 elements in one response.

The crate already implements this rule twice, in both of its other paginated reads, each deriving the constant from the same kind of member doc: `GetCreditAllocationHistory` applies `DEFAULT_MAX_RESULTS = 100` and `GetBillingPreferences` applies `DEFAULT_MAX_RESULTS = 50` when the request omits the member. `ListBillingViews` (and `ListSourceViewsForBillingView`, which shares the same `_ =>` shape) is the odd one out: it treats an omitted page size as "no page size".

## Evidence

The documented default, quoted in the handler's own module doc — services/service-com-amazonaws-billing-awsbilling/src/ops/list_billing_views.rs:64-65:
```
//! * `maxResults` (BillingViewsMaxResults, range 1..100) — The maximum number of billing
//!   views to retrieve. Default is 100.
```
The code that ignores it — services/service-com-amazonaws-billing-awsbilling/src/ops/list_billing_views.rs:358-367:
```
        // `maxResults` is the page size; absent, the whole result set is one page.
        let next_token = match input.max_results.and_then(|max| usize::try_from(max).ok()) {
            Some(limit) if matched.len() > limit => {
                matched.truncate(limit);
                matched
                    .last()
                    .map(|view| crate::state::encode_page_token(&view.arn))
            }
            _ => None,
        };
```
The sibling paginated reads of this same crate that DO apply their documented default — services/service-com-amazonaws-billing-awsbilling/src/ops/get_credit_allocation_history.rs:151-155 and 354-358:
```
/// The page size used when the request omits `maxResults`.
///
/// From the `maxResults` doc: "The maximum number of records to return per
/// page. Range: 1 to 1000. Default: 100."
const DEFAULT_MAX_RESULTS: usize = 100;
...
        let limit = input
            .max_results
            .and_then(|max| usize::try_from(max).ok())
            .unwrap_or(DEFAULT_MAX_RESULTS)
```
and services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_preferences.rs:139-141 / 320-323:
```
/// The page size a request that names none asks for: "The maximum number of
/// records to return per page. Range: 1 to 50. Default: 50." (`maxResults` doc).
const DEFAULT_MAX_RESULTS: usize = 50;
...
        let limit = input
            .max_results
            .and_then(|max| usize::try_from(max).ok())
            .unwrap_or(DEFAULT_MAX_RESULTS);
```
Test coverage: services/service-com-amazonaws-billing-awsbilling/tests/all/billing_views.rs:513-529 (`pagination_returns_every_view_exactly_once_for_every_page_size`) calls ListBillingViews with no `maxResults` and uses the answer as the full expected set, but it creates only a handful of views, so applying a 100-entry default leaves that test's answer unchanged.

## Steps to Reproduce

1. Boot the emulator and point an awsJson1_0 `billing` client at it.
2. Create 120 custom billing views with CreateBillingView (the store admits up to MAX_BILLING_VIEWS = 256).
3. Call ListBillingViews with an empty body (no `maxResults`).
4. Observe `billingViews` carries all 121 entries and `nextToken` is absent, instead of the documented default page of 100 plus a `nextToken` — while GetCreditAllocationHistory and GetBillingPreferences on the same emulator do cap an omitted `maxResults` at their documented defaults.
