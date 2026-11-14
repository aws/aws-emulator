Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9d2e170c3a479b68290b0c2532ee0534a60c9186

# Fold the two hand-rolled hex page-token codecs onto the crate's existing state::encode_page_token/decode_page_token

_Filed: 2026-11-01T11:37:09Z_
_Operation: ListSourceViewsForBillingView | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9d2e170c3a479b68290b0c2532ee0534a60c9186`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate already owns one page-token codec — `crate::state::encode_page_token` (hex of the cursor's bytes) and `crate::state::decode_page_token` (hex decode + UTF-8) — and `ListBillingViews` and `GetCreditAllocationHistory` both call it. `ListSourceViewsForBillingView` and `GetBillingPreferences` re-implement the identical hex+UTF-8 half locally instead of calling it, each adding only its own payload parse on top (a decimal position; a `feature:count` pair). The same two-line transcoding rule therefore exists in three places in this crate's own code and can drift: a change to the token transcoding (e.g. rejecting mixed-case hex spellings the encoder never mints) applied in state.rs would silently leave these two reads on the old rule. Both local copies can be expressed as `crate::state::encode_page_token(&payload)` / `crate::state::decode_page_token(token)` followed by the operation's own parse, leaving every response byte and every existing test unchanged.

## Evidence

The crate's shared codec — services/service-com-amazonaws-billing-awsbilling/src/state.rs:1148-1163:
```
pub fn encode_page_token(last: &str) -> String {
    ::hex::encode(last.as_bytes())
}
...
pub fn decode_page_token(token: &str) -> Option<String> {
    ::hex::decode(token)
        .ok()
```
called by services/service-com-amazonaws-billing-awsbilling/src/ops/list_billing_views.rs:206-207 (`crate::state::decode_page_token(token).ok_or_else(...)`) and services/service-com-amazonaws-billing-awsbilling/src/ops/get_credit_allocation_history.rs:199-200.
Copy 1 — services/service-com-amazonaws-billing-awsbilling/src/ops/list_source_views_for_billing_view.rs:149-161:
```
fn encode_page_token(returned: usize) -> String {
    hex::encode(returned.to_string().as_bytes())
}
...
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| text.parse::<usize>().ok())
```
(its own doc already points at the shared module for the refusal wording: "The refusal's wording is the one in `crate::state`, shared with this service's other paginated reads", list_source_views_for_billing_view.rs:154-156.)
Copy 2 — services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_preferences.rs:165-178:
```
fn encode_page_token(feature: &str, consumed: usize) -> String {
    hex::encode(format!("{feature}:{consumed}"))
}
...
    let decoded = hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
```
Tests that cover the behaviour and must stay green unchanged: tests/all/source_views.rs:284-334 (hex/non-hex/negative-position tokens -> 400 ValidationException, a token past the end -> 200 empty page) and `source_view_pagination_returns_every_source_once_for_every_page_size` (tests/all/source_views_tags_and_policy.rs:516-544); tests/all/billing_preferences.rs:717 ("text that is not a token" -> 400); tests/all/credit_allocation_history.rs:776 and the ListBillingViews pagination tests for the shared-helper callers.

## Steps to Reproduce

1. Read services/service-com-amazonaws-billing-awsbilling/src/state.rs:1148-1163 — the crate's page-token encode/decode pair.
2. Read services/service-com-amazonaws-billing-awsbilling/src/ops/list_billing_views.rs:206 and src/ops/get_credit_allocation_history.rs:200 — two reads that call it.
3. Read src/ops/list_source_views_for_billing_view.rs:149-173 and src/ops/get_billing_preferences.rs:165-199 — the same hex+UTF-8 transcoding written out again, with only the payload parse differing.
4. Note that a change to the shared codec (token minting/acceptance rule) would not reach either local copy.
