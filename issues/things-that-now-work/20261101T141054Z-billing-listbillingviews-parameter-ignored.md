Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 043b7f8056dcef57c0c9baaf75abb2dd363a8bd0

# The sourceAccountId filter matches against a stored field no writer ever populates, so it always selects nothing

_Filed: 2026-11-01T14:10:54Z_
_Operation: ListBillingViews | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `043b7f8056dcef57c0c9baaf75abb2dd363a8bd0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListBillingViews` implements the `sourceAccountId` filter as an equality test against `BillingView::source_account_id`. That field is assigned exactly twice in the crate — `None` for the seeded PRIMARY view and `None` for every view `CreateBillingView` mints — and is never set to an account id anywhere, in `src/` or in the tests. The filter is therefore unsatisfiable: for any 12-digit value a caller sends, `ListBillingViews` answers an empty `billingViews` page, and `BillingViewListElement.sourceAccountId` / `BillingViewElement.sourceAccountId` are always null.

The model documents the filter in terms of a view's SOURCES — "billing views that use the specified account as a source" — and the sources are state this crate does hold: `BillingView::source_views` is a non-empty list of billing-view ARNs (`sourceViews` is `@required`, `@length` 1..10), and `state::arn_account_id` already extracts the account id an ARN embeds. The crate derives the other source-dependent projections from `source_views` on every read for exactly this reason (`GetBillingView`: "both are derived from the stored records on every read, never kept as a copy that could go stale"); this one filter instead reads a copy nothing writes.

## Evidence

Smithy (`smithy_query(query="doc", shape="ListBillingViewsRequest", member="sourceAccountId")`): "Filters the results to include only billing views that use the specified account as a source."
Smithy (`shape="BillingViewListElement", member="sourceAccountId"`): "The Amazon Web Services account ID that owns the source billing view, if this is a derived billing view."

The filter — services/service-com-amazonaws-billing-awsbilling/src/ops/list_billing_views.rs:241-245:
```
    if let Some(source_account) = &input.source_account_id {
        if view.source_account_id.as_deref() != Some(source_account.as_str()) {
            return Ok(false);
        }
    }
```
The only two writers of that field, both hard-coded `None` — services/service-com-amazonaws-billing-awsbilling/src/ops/create_billing_view.rs:257-259:
```
            owner_account_id: ::engine::emulator_account_id().to_string(),
            source_account_id: None,
            source_views: input.source_views,
```
and services/service-com-amazonaws-billing-awsbilling/src/state.rs:588: `source_account_id: None,` (the seeded PRIMARY view). `grep -rn source_account_id services/service-com-amazonaws-billing-awsbilling` shows no other assignment — only the field declaration (state.rs:320) and three read sites (list_billing_views.rs:242, :377, get_billing_view.rs:214).

The state the crate does keep, and the helper that would resolve it — services/service-com-amazonaws-billing-awsbilling/src/state.rs:321-322 (`/// The billing-view ARNs this view draws its data from.` `pub source_views: Vec<String>,`) and state.rs:990-993:
```
/// The account id an ARN embeds (its fifth colon-separated segment), when it
/// has one.
pub fn arn_account_id(arn: &str) -> Option<&str> {
```
The crate's own rule for source-derived projections — services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_view.rs:190-193: "`sourceViewCount` is \"the number of source views associated with this billing view\" ... both are derived from the stored records on every read, never kept as a copy that could go stale."

Test coverage: tests/all/billing_views.rs:481-485 pins the contradicting answer as a bare table row ("sourceAccountId selects nothing when no view is derived", expected `vec![]`) even though the four views the row runs against were each created naming an own-account view as a source.

## Steps to Reproduce

1. Boot the emulator with the caller recorded as the organization management account so CreateBillingView succeeds.
2. CreateBillingView(name="derived", sourceViews=["arn:aws:billing::000000000000:billingview/primary"]) — 200.
3. Call ListBillingViews with {"sourceAccountId":"000000000000"}.
4. Observe `billingViews: []`, although the view created in step 2 uses a billing view of account 000000000000 as its source; also observe that GetBillingView on that view reports no `sourceAccountId` at all.
5. Any other 12-digit value gives the same empty page: the filter can never select a view.
