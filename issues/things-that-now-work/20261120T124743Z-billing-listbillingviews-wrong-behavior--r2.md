Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 233e85f7211280754246a7d02478fa1f321c9471

# An unfiltered ListBillingViews reports billing views of OTHER accounts, the same records the same handler answers 403 for when the request names their ARN

_Filed: 2026-11-20T12:47:43Z_
_Operation: ListBillingViews | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `233e85f7211280754246a7d02478fa1f321c9471`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListBillingViews` walks `state.billing_views.values()` with no account predicate at all: the only account scoping it applies is the OPTIONAL `ownerAccountId` filter and the `arns` filter's boundary check. So a billing view stored under an ARN of another account is reported in full — `arn`, `name`, `description`, `ownerAccountId`, `billingViewType`, `healthStatus` — to a caller that simply omits the filters.

The same handler refuses to say anything at all about that very record when the request NAMES it: `check_arns_filter_accounts` refuses every `arns` entry outside the caller's account with `AccessDeniedException` (403) before the store is even locked, and the handler's own module doc states that rule. The crate states the same rule crate-wide on `state::is_caller_account_arn` ("so the answer to a foreign-account ARN never depends on whether a view exists there") and pins it as an API-wide property in two test suites, one of which plants a record under each foreign ARN precisely so that "the existence of a resource in another account is not observable through this API" can be asserted — and then asserts `!rendered.contains(PLANTED_MARKER)` for every ARN-addressed operation. The unfiltered listing is the one read in the crate that hands those planted fields straight back, so the crate contradicts itself about the same stored record: named, it is 403 and discloses nothing; unnamed, it is a full list entry.

(Foreign-account views are a state this crate's own test harness creates — `CreateBillingView` only ever mints ARNs in the caller's account — which is exactly the state the cross-account suite exists to cover.)

## Evidence

The rule, in this handler's own module doc — services/service-com-amazonaws-billing-awsbilling/src/ops/list_billing_views.rs:17-19:
```
//! The `arns` filter names resources, so each entry is resolved first: one
//! outside the caller's account is an `AccessDeniedException` (403) and one this
//! account stores no view under is a `ValidationException` (400).
```
...and on the helper that enforces it — same file:178-196:
```
/// Enforce the account boundary on every entry of the `arns` filter, BEFORE the
/// store is locked: ...
fn check_arns_filter_accounts<Eng: Send + Sync + 'static>(
    arns: &[String], engine: &Eng, ctx: &smithy_server_runtime::OperationContext<'_>,
) -> Result<(), ListBillingViewsError> {
    for arn in arns {
        if !crate::state::is_caller_account_arn(arn) {
            return Err(ListBillingViewsErrorModeled::AccessDeniedException(
                crate::state::cross_account_access_denied("ListBillingViews", arn, engine, ctx),
```
The walk that ignores it — same file:419-432:
```
        let mut matched: Vec<&crate::state::BillingView> = Vec::new();
        // `billing_views` is a BTreeMap keyed by ARN, so this walk is in ARN
        // order and the page boundary is stable.
        for view in state.billing_views.values() {
            ...
            if matches_filters(view, &input)? {
                matched.push(view);
            }
        }
```
and the only owner predicate, which fires only when the request carries one — same file:307-311:
```
    if let Some(owner) = &input.owner_account_id {
        if *owner != view.owner_account_id {
            return Ok(false);
        }
    }
```
The crate-wide rule — services/service-com-amazonaws-billing-awsbilling/src/state.rs:1444-1451:
```
/// Whether `arn` is scoped to the caller's own account.
///
/// Every operation that identifies a billing view by ARN checks this BEFORE it
/// looks the ARN up, so the answer to a foreign-account ARN never depends on
/// whether a view exists there.
pub fn is_caller_account_arn(arn: &str) -> bool {
```
The property the suite claims for the whole API — services/service-com-amazonaws-billing-awsbilling/tests/all/cross_account_existence_signal.rs:6-11:
```
//! One rule: an operation that identifies a resource of this service by ARN
//! decides the caller-account boundary BEFORE it consults the store, so a
//! well-formed ARN whose embedded account id is not the caller's is
//! `AccessDeniedException` (403) and the answer carries nothing a record stored
//! under that ARN would have contributed — the existence of a resource in another
//! account is not observable through this API.
```
and the assertion it makes about planted foreign records — same file:150-156:
```
            assert!(
                !rendered.contains(PLANTED_MARKER),
                "case {label:?}: no part of the stored record may reach the answer, got \
                 {rendered:?}"
            );
```
Test coverage: that loop (cross_account_existence_signal.rs:126-157) drives only the tables in tests/all/arn_account_boundary.rs:39-69, where `ListBillingViews` appears ONLY as `|arn| json!({ "arns": [arn] })` (arn_account_boundary.rs:47) — no test calls `ListBillingViews` with no `arns` filter against a store holding a foreign-account view, so nothing pins or justifies the disclosure.

## Steps to Reproduce

1. Stand the emulator up with the harness in tests/all/billing_views.rs::emulator_with_engine.
2. Plant a billing view under a foreign ARN exactly as tests/all/cross_account_existence_signal.rs:63-78 does, e.g. arn:aws:billing::111122223333:billingview/primary with name/description/tags carrying a recognisable marker.
3. Call ListBillingViews with {"arns": ["arn:aws:billing::111122223333:billingview/primary"]}: 403 AccessDeniedException, and (per cross_account_existence_signal.rs:150-156) no field of the stored record reaches the body.
4. Call ListBillingViews with {} (no filters): the answer's billingViews contains an entry for that same foreign ARN carrying its name, description, ownerAccountId and healthStatus — the record the previous call refused to disclose.
5. Compare with src/state.rs:1444-1451 and src/ops/list_billing_views.rs:17-19: the crate states a foreign-account record is not observable, and this one read makes it observable.
