Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a1183bf81eedba070a3bea65bd1e1c8aae3c4a55

# A BENEFIT_LEVEL_SHARING read that omits filters is answered with every stored key, although the modelled filters doc states the filter is required for that feature

_Filed: 2026-11-28T04:25:27Z_
_Operation: GetBillingPreferences | Class: validation_

> _Note: this issue was identified against the code base at commit `a1183bf81eedba070a3bea65bd1e1c8aae3c4a55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetBillingPreferencesRequest$filters` documents one per-feature rule: for `BENEFIT_LEVEL_SHARING` the filter is REQUIRED, and its single value has to identify the Reserved Instance or Savings Plan in the form `benefit/{ri|sp}/{benefitId}`. The handler applies neither half.

`validate_filters` only rejects a filter whose `name` is not `PREFERENCE_KEY`; it never looks at which feature the request names, so a request carrying `features: ["BENEFIT_LEVEL_SHARING"]` and no `filters` at all passes. `filtered_keys` then answers `None` for a request that carries no filter ("Absent `filters` ... each select everything"), and `matches_filters` turns that `None` into "every entry matches" — so the read answers HTTP 200 with every `BENEFIT_LEVEL_SHARING` key the store holds, for every benefit, instead of refusing the request with the modelled `ValidationException` the operation declares.

The same gate is the only place the documented value FORM could be checked: a `filters` entry whose value is not `benefit/{ri|sp}/{benefitId}` (e.g. `open-sharing`) is accepted and simply matches no key, which a caller cannot tell from "that benefit has no preferences".

This is the read-side half of a rule the crate does not implement anywhere: the write side carries its own open issue, and `BENEFIT_LEVEL_SHARING` appears in no test and in no handler branch of this crate.

## Evidence

Model, the sentence that states the rule itself — smithy_query(query="shape", shape="GetBillingPreferencesRequest"), member `filters`:
```
"filters": {"target": "BillingFeatureFilters", "length": {"min": 1, "max": 1}, "type": "list", "doc": "<p>Filters to narrow results. Specify exactly one filter when supplied. The supported filter name is <code>PREFERENCE_KEY</code>, which accepts 1 to 10 values to match preference keys.</p> <p>For <code>BENEFIT_LEVEL_SHARING</code>, this filter is required. Specify exactly one value that identifies the Reserved Instance (RI) or Savings Plan (SP), in the form <code>benefit/{ri|sp}/{benefitId}</code>.</p>"}
```
(the member is NOT `@required` on the shape, so the boundary validator cannot see this rule — it is the handler's to apply)

The only filter validation the handler performs — services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_preferences.rs:292-302:
```
fn validate_filters(input: &GetBillingPreferencesInput) -> Result<(), GetBillingPreferencesError> {
    for filter in input.filters.iter().flatten() {
        if !is_preference_key_filter(filter) {
            return Err(invalid_field(
                "filters",
                "name is not a filter this service supports",
            ));
        }
    }
    Ok(())
}
```
Note the loop body runs only over filters the request CARRIES (`input.filters.iter().flatten()`), so a request with no `filters` member validates vacuously, whichever feature it names.

What an absent filter then selects — get_billing_preferences.rs:304-312 (doc) and 317-325:
```
/// The preference keys the request's `filters` select, or `None` when they
/// narrow nothing.
///
/// Absent `filters`, and a filter carrying no value, each select everything —
/// a filter that names nothing to match narrows nothing, ...
fn filtered_keys(input: &GetBillingPreferencesInput) -> Option<BTreeSet<&str>> {
    let mut selected: Option<BTreeSet<&str>> = None;
    for filter in input.filters.iter().flatten() {
```
...and get_billing_preferences.rs:336-341:
```
fn matches_filters(
    entry: &crate::state::PreferenceEntry,
    selected: Option<&BTreeSet<&str>>,
) -> bool {
    selected.is_none_or(|keys| keys.contains(entry.key.as_str()))
}
```

The handler's own call sequence — get_billing_preferences.rs:409-437 — reads the one `feature`, calls `validate_filters(&input)?`, then `filtered_keys(&input)`; nothing between them relates the feature to the presence of a filter:
```
        let Some(feature) = input.features.first() else {
            return Err(invalid_field("features", "specify exactly one feature"));
        };
        validate_filters(&input)?;
...
        let selected = filtered_keys(&input);
```

Test coverage: NONE pins this. `grep -r BENEFIT_LEVEL_SHARING services/service-com-amazonaws-billing-awsbilling` matches only OPERATIONS.md:249, src/ops/types/shared_types.rs:186 (`BILLING_FEATURE_VALUES`) and the raw model — no file under tests/all/ mentions the feature or a `benefit/` key, so no test states a reason for answering such a request. The feature is also reachable: it is not among `ACCESS_ACTIVATION_FEATURES`, so `has_feature_access("BENEFIT_LEVEL_SHARING")` is true by default and the request reaches the filter logic rather than the 403 gate above it.

## Steps to Reproduce

1. Boot the emulator with this service mounted (the awsJson1_0 harness in tests/all/billing_preferences.rs).
2. Store a BENEFIT_LEVEL_SHARING preference first, so the feature has keys to report: POST / with `X-Amz-Target: AWSBilling.UpdateBillingPreferences` and body {"feature":"BENEFIT_LEVEL_SHARING","billingPreferencesPerKey":[{"key":"benefit/ri/AAA/sharing/mode/prioritized","value":"ENABLED"}]}
3. POST / with `X-Amz-Target: AWSBilling.GetBillingPreferences` and body {"features":["BENEFIT_LEVEL_SHARING"]} — no `filters` member at all.
4. Observe HTTP 200 carrying every stored BENEFIT_LEVEL_SHARING key, where the modelled `filters` doc states "For BENEFIT_LEVEL_SHARING, this filter is required" — i.e. the modelled ValidationException (400) this operation declares.
5. Repeat with {"features":["BENEFIT_LEVEL_SHARING"],"filters":[{"name":"PREFERENCE_KEY","value":["open-sharing"]}]} and observe HTTP 200 with an empty `billingPreferences` list, although the value identifies no RI/SP in the documented `benefit/{ri|sp}/{benefitId}` form.
