Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 9d2e170c3a479b68290b0c2532ee0534a60c9186

# ListSourceViewsForBillingView resolves a foreign-account ARN against the store, answering 404 where GetBillingView answers 403

_Filed: 2026-11-01T11:37:09Z_
_Operation: ListSourceViewsForBillingView | Class: error-response_

> _Note: this issue was identified against the code base at commit `9d2e170c3a479b68290b0c2532ee0534a60c9186`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetBillingView` — the other read of this crate that resolves a single `BillingViewArn` — enforces the account boundary before any existence lookup, with a comment stating that this service never reveals whether a view exists in another account. `ListSourceViewsForBillingView` takes the same `arn` member and declares the same `AccessDeniedException` (HTTP 403), but goes straight to `state.billing_views.get(&input.arn)` and maps a miss to `ResourceNotFoundException` (404) with the ARN echoed into the message. A caller probing foreign-account ARNs therefore gets the existence signal through this operation that the sibling read deliberately withholds, and a client branching on `AccessDeniedException` takes its not-found path instead.

## Evidence

Sibling read that gates first — services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_view.rs:159-174:
```
// The account boundary is enforced BEFORE any existence lookup, so this
// service never reveals whether a view exists in another account: a
// well-formed ARN whose embedded account id is not the caller's own is
// refused with the modeled `AccessDeniedException` (403) whatever is
// stored.
if !crate::state::is_caller_account_arn(&input.arn) {
    return Err(GetBillingViewErrorModeled::AccessDeniedException(
        crate::state::cross_account_access_denied(Self::NAME, &input.arn, &*self.engine, ctx),
    )
    .into());
}
```
The handler under review — services/service-com-amazonaws-billing-awsbilling/src/ops/list_source_views_for_billing_view.rs:205-226:
```
let _ = ctx;
let state = crate::state::service_state(&*self.engine).ok_or_else(...)?;
let state = state.lock();

let view = state.billing_views.get(&input.arn).ok_or_else(|| {
    ListSourceViewsForBillingViewErrorModeled::ResourceNotFoundException(
        ResourceNotFoundException {
            message: crate::state::invalid_billing_view_arn(&input.arn),
```
The model gives this operation the 403 it never answers — OPERATIONS.md:143 for ListSourceViewsForBillingView: "**Errors:** `AccessDeniedException (HTTP 403)`, …".
The crate's own statement of the rule, in the refusal suite's module doc — services/service-com-amazonaws-billing-awsbilling/tests/all/rejection_fidelity.rs:12-14: "2. An operation that identifies a billing view by ARN enforces the account\n//!    boundary BEFORE any existence lookup, so an ARN outside the caller's own\n//!    account is `AccessDeniedException` (403) whatever is stored." — and its table at rejection_fidelity.rs:336-341 does not include this operation. The existing 404 coverage (tests/all/source_views.rs:453-481, `list_source_views_tells_a_missing_view_from_one_with_no_sources`) only uses own-account ARNs, so no test pins the foreign-account answer.

## Steps to Reproduce

1. Boot the emulator and point an awsJson1_0 `billing` client at it.
2. Call GetBillingView(arn="arn:aws:billing::111122223333:billingview/primary") — observe 403 AccessDeniedException.
3. Call ListSourceViewsForBillingView(arn="arn:aws:billing::111122223333:billingview/primary") — observe 404 ResourceNotFoundException with message "[arn:aws:billing::111122223333:billingview/primary] is an invalid billing view arn", instead of the modeled 403.
