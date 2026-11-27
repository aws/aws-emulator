Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 86a81857af943b6387678f8429ed876a9a758ecc

# Nothing in src/ ever records a billing-view segment, so every ListBillingViewSegments answer is the one seeded open-ended BILLABLE segment

_Filed: 2026-11-24T03:32:23Z_
_Operation: ListBillingViewSegments | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `86a81857af943b6387678f8429ed876a9a758ecc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListBillingViewSegments` reports `ComAmazonawsBillingAwsbillingState::billing_view_segments`. The ONLY write to that map anywhere in `src/` is the store's `Default` seed, which inserts a single `BillingViewSegment::in_effect(primary.arn, BILLING_DOMAIN_BILLABLE)` — unbounded on both sides, with all three account-relationship members absent. The writer the state module provides for everything else, `record_billing_view_segment`, has no caller in `src/` at all (only three test files call it), and `CreateBillingView` inserts into `billing_views` without recording any timeline for the view it mints.

The consequence over the wire is that no sequence of API calls can change what this read answers: `items` is always exactly one element, `domain` is always `BILLABLE`, its `timeRange` is always exactly the window the request asked about (the stored record is unbounded, so `BillingViewSegment::clipped` hands back the request's own bounds), `billingTransferAccountId` / `managementAccountId` / `billingGroupPrimaryAccountId` are always absent, and `nextToken` is never present. The handler's own documented behaviour — several segments for a view whose relationships changed mid-period, the omission of hidden records, and the whole scoped-token pagination walk — is unreachable for every request a client can make.

This is the same class of gap as the already-filed "nothing in src/ ever records Support-eligible spend" and "nothing in src/ ever stores a billing-view resource policy" issues, for a different store.

## Evidence

The handler doc states the rule the code under it can never reach — services/service-com-amazonaws-billing-awsbilling/src/ops/list_billing_view_segments.rs:7-11:

    //! Each reported segment is one record of that timeline, its period
    //! clipped to the requested one, carrying the billing domain and the account
    //! relationships that held over it; a view whose relationships changed
    //! mid-period therefore answers with several segments, and a hidden record is
    //! omitted, so the answer can cover less than the requested period.

The state module states the same rule — src/state/billing_view_segments.rs:10-13:

    //! A mid-period change is two records — the earlier one closed at the
    //! instant of the change, the later one opened there — which is what makes
    //! a single request answer with several segments, each with its own time range.

The only write in src/ — src/state.rs:828-831:

    let primary_segment =
        BillingViewSegment::in_effect(&primary.arn, BILLING_DOMAIN_BILLABLE);
    let mut billing_view_segments = BTreeMap::new();
    billing_view_segments.insert(primary_segment.key(), primary_segment);

with its justification frozen at store-construction time — src/state.rs:821-827: "the account belongs to no organization (see `organization_management_account_id`, the fact read from the Organizations store), no billing group and no billing transfer by default, so the segment carries no account relationship and is bounded on neither side" — while `is_organization_management_account` shows the crate reads that Organizations fact at REQUEST time for `CreateBillingView` (src/ops/create_billing_view.rs:240).

The provided writer has no production caller — src/state.rs:999 `pub fn record_billing_view_segment(&mut self, segment: BillingViewSegment) -> bool`; grep for it in the repo returns only its own definition in src/ plus tests/all/token_scope.rs:218, tests/all/billing_view_segments.rs:231 and tests/all/billing_view_delete_cascade.rs:513.

The crate's state doc claims every view has a timeline — src/state.rs:9-13: "Each view also has a **segment timeline** in [`ComAmazonawsBillingAwsbillingState::billing_view_segments`] — the periods over which its billing domain and account relationships did not change, which `ListBillingViewSegments` reports" — but the create records none: src/ops/create_billing_view.rs:357 `state.billing_views.insert(arn.clone(), view);` is the whole of what it persists.

Model (smithy_query shape=BillingViewSegmentsListElement): "A segment represents a time range during which the billing domain and account relationships for a billing view remained unchanged"; `managementAccountId` — "The management account ID of the organization. The response includes this field for organization member accounts."; `billingGroupPrimaryAccountId` — "The billing group primary account ID. The response includes this field for billing group members." None of the three members can ever be populated by this emulator.

Test coverage: tests/all/billing_view_segments.rs, tests/all/billing_view_segment_future_period.rs and tests/all/token_scope.rs all reach multi-segment behaviour only by writing records directly into the store through `record_billing_view_segment`, i.e. through a path no request can take.

## Steps to Reproduce

1. Boot the emulator and call ListBillingViewSegments with an empty body (no arn, no timeRange). Note the answer: exactly one `items` entry, `domain: BILLABLE`, `timeRange` equal to the current UTC calendar month, no `managementAccountId`/`billingGroupPrimaryAccountId`/`billingTransferAccountId`, no `nextToken`.
2. Exercise every modelled operation of this service in any order (CreateBillingView / AssociateSourceViews / DisassociateSourceViews / UpdateBillingView / TagResource / UpdateBillingPreferences / RedeemCredits ...), then call ListBillingViewSegments again with any timeRange inside the month: the answer is byte-for-byte the same single segment, because no handler writes to `billing_view_segments`.
3. Confirm the absent writer: `grep -rn record_billing_view_segment services/service-com-amazonaws-billing-awsbilling/src` returns only the definition at src/state.rs:999 (callers are tests only).
4. Call CreateBillingView, then read `engine.state::<ComAmazonawsBillingAwsbillingState>().lock().billing_view_segments`: the new view has no segment record at all, although src/state.rs:9-13 states "Each view also has a segment timeline".
