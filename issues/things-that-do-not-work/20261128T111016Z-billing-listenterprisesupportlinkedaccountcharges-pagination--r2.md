Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 25bee91db5277731939dc67c98f1a63f411c98ed

# The nextToken scope omits the accountId filter, so a token minted by a narrowed request silently drops the month's other linked accounts

_Filed: 2026-11-28T11:10:16Z_
_Operation: ListEnterpriseSupportLinkedAccountCharges | Class: pagination_

> _Note: this issue was identified against the code base at commit `25bee91db5277731939dc67c98f1a63f411c98ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListEnterpriseSupportLinkedAccountCharges` pages the `support_eligible_spend` store, whose keys are `"<billingMonth>\u{1f}<accountId>"`. The request's `accountId` filter is applied to the walk (`selected = ... .filter(|(_, spend)| input.account_id ... == spend.account_id)`), so it decides WHICH of the month's bills the walk holds — yet the token scope this read binds its `nextToken` to is `[(SCOPE_BILLING_MONTH, input.billing_month)]` only. `accountId` never travels in the token and is never compared with the resuming request.

Consequence: a token minted while paging a NARROWED selection is accepted by an unnarrowed request of the same month and applied as its cut point. The cursor is the key of the last bill of the filtered subsequence — a key that sorts above every bill of the month whose account id sorts below the filtered account — so the resumed, unfiltered page reports only keys strictly greater than it and silently drops every one of the month's other accounts, answering HTTP 200. Those bills were never reported by the walk the token came from.

This is NOT the gap the fixed issue 20261115T233241Z-billing-listenterprisesupportlinkedaccountcharges-pagination closed: that one added `billingMonth` to the scope, and only the month. Every other filtered paginated read in this same crate puts each narrowing member in the token scope, and the closest sibling — which walks the SAME store with the SAME two input members — names `accountId` in its scope for exactly this reason. This read is the one that does not.

The crate's own test for this read asserts only the SAFE direction (a token minted with no `accountId`, replayed with one — the filtered walk is a subsequence of the unfiltered one, so no entry is skipped). The unsafe direction (token minted WITH `accountId`, replayed without) is never exercised, so the suite's docstring claim that "the account filter narrows one and the same walk, so a cursor stays a cut point of it" is pinned by no assertion.

## Evidence

The scope this read binds its token to carries the month and nothing else — services/service-com-amazonaws-billing-awsbilling/src/ops/list_enterprise_support_linked_account_charges.rs:411-416:
```
        let page_key = state.page_token_key();
        let scope = [(SCOPE_BILLING_MONTH, input.billing_month.as_str())];
        let resume_after = match input.next_token.as_deref() {
            Some(token) => Some(resume_key(&page_key, Self::NAME, token, &scope)?),
            None => None,
        };
```
...while the `accountId` filter does narrow the walk — list_enterprise_support_linked_account_charges.rs:428-435:
```
        let selected = state
            .enterprise_support_spend_entries_for_month(&payer_account_id, &input.billing_month)
            .filter(|(_, spend)| {
                input
                    .account_id
                    .as_deref()
                    .is_none_or(|filter| filter == spend.account_id)
            });
```
...and the cursor is compared straight against those keys — list_enterprise_support_linked_account_charges.rs:441-450:
```
        let mut remaining =
            selected.filter(|(key, _)| resume_after.as_ref().is_none_or(|after| *key > after));
        let page: Vec<(&String, &SupportEligibleSpend)> = remaining.by_ref().take(limit).collect();
        let next_token = if remaining.next().is_some() {
            page.last().map(|(key, _)| {
                crate::state::encode_scoped_page_token(&page_key, Self::NAME, &scope, key)
            })
```
The key whose second segment is the account — services/service-com-amazonaws-billing-awsbilling/src/state/support_charges.rs:160-162:
```
    pub fn key(&self) -> String {
        format!("{}\u{1f}{}", self.billing_month, self.account_id)
    }
```
The SIBLING read of the SAME store with the SAME two members, which DOES scope on the filter — services/service-com-amazonaws-billing-awsbilling/src/ops/list_business_support_account_charges.rs:539-555:
```
        // The key the store's tokens are authenticated under, taken off the
        // store by value so the walk below can borrow the records it reports.
        // The members the request names select WHICH bills are walked — the
        // month, because the store is keyed month first, so a cut point of
        // another month is a key of another sequence, and the `accountId`
        // filter, because it decides which of the month's bills the walk holds
        // at all — so every token of this walk carries both and one minted under
        // another selection is refused rather than compared against the keys of
        // this one.
        let page_key = state.page_token_key();
        let scope = [
            (SCOPE_BILLING_MONTH, input.billing_month.as_str()),
            (
                SCOPE_ACCOUNT_FILTER,
                account_filter_scope(input.account_id.as_deref()),
            ),
        ];
```
...and the sibling's own statement of the failure mode this read exhibits — list_business_support_account_charges.rs:331-345:
```
/// the token is no cut point of the sequence THIS request selects: ... and one
/// it minted while paging another selection of
/// bills — another billing month, whose keys are that month's and name no bill
/// of this one, or another account filter, whose walk holds bills this one does
/// not.
///
/// A cursor is a position among the entries the request selects, so a token is
/// usable only by a request that selects the same ones: comparing a cut point of
/// another selection against these keys would drop every bill of this walk that
/// sorts at or below it — bills the walk the token came from never reported —
/// and answer that truncated page with a 200.
```
The same crate-wide rule in this read's other siblings: `GetCreditAllocationHistory` (get_credit_allocation_history.rs:425-429, scope = month range + account + credit filter), `ListBusinessSupportSubscriptionHistory` (list_business_support_subscription_history.rs:394-400, scope = resolved period + account filter), `ListBillingViews` (list_billing_views.rs:537-550, scope = arns + types + names + owner + source account + active time range), `GetBillingPreferences` (get_billing_preferences.rs:343-371, `filter_scope` digests the filtered keys), `ListBillingViewSegments` (list_billing_view_segments.rs:493-496, scope = view + time range), `ListSourceViewsForBillingView` (list_source_views_for_billing_view.rs:236-241, scope = the view ARN).

Test coverage: tests/all/enterprise_support_linked_account_charges_token_scope.rs mints EVERY replayed token from an unfiltered request — `first_token` at :195-198 calls `json!({ "billingMonth": billing_month, "maxResults": 1 })` with no `accountId` — so its two `accountId` rows (:335-343) only exercise unfiltered-token -> filtered-request, the direction that cannot skip anything. No test replays a token minted WITH an `accountId` against a request without one. The suite's docstring claim is at :16-17: "`maxResults` is no part of a selection, and neither is `accountId`: ... the account filter narrows one and the same walk, so a cursor stays a cut point of it."

## Steps to Reproduce

1. Stand up the emulator with the caller as the billing family's payer account (the harness `standalone_emulator_with_engine` used by tests/all/enterprise_support_linked_account_charges_token_scope.rs).
2. Seed Enterprise Support spend (`support_plan_name = AWSSupportEnterprise`) for three family accounts in one past month, e.g. 2024-03 for 000000000000 (the caller), 111111111111 and 222222222222 — the `seed` helper in that suite does exactly this.
3. Call ListEnterpriseSupportLinkedAccountCharges({"billingMonth":"2024-03"}) and observe all three accounts reported, ascending.
4. Call ListEnterpriseSupportLinkedAccountCharges({"billingMonth":"2024-03", "accountId":"222222222222", "maxResults":1}) — one entry, and a nextToken is minted whose cursor is the key "2024-03\u{1f}222222222222" (a token is minted because the filtered walk still has... if it does not, use accountId 111111111111 with maxResults 1 against a two-account filter; the cursor is in every case the key of the last entry of the NARROWED walk).
5. Replay that token with the unnarrowed request: ListEnterpriseSupportLinkedAccountCharges({"billingMonth":"2024-03", "nextToken":<that token>}).
6. Observe HTTP 200 with `linkedAccount` holding only the accounts sorting ABOVE the cursor — 000000000000 and 111111111111 are silently dropped — instead of the modeled ValidationException the sibling ListBusinessSupportAccountCharges answers for a token minted under another account filter.
