Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: d12a09599b9528aaa3eec06d793185a816ce0c56

# An account-list preference key is resolved against the consolidated billing family only when it is a sharing key, so a non-sharing feature stores a key naming an account the family does not hold

_Filed: 2026-11-21T13:27:32Z_
_Operation: UpdateBillingPreferences | Class: validation_

> _Note: this issue was identified against the code base at commit `d12a09599b9528aaa3eec06d793185a816ce0c56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler's own module doc states the rule for EVERY key that names a resource: an account-list key's trailing account is resolved against the consolidated billing family, and "A key naming a resource the family does not hold is refused 403 AccessDeniedException and stores nothing."

The code implements that resolution only inside the sharing-key branch. The per-entry loop first does `if !crate::state::is_sharing_key(feature, &preference.key) { continue; }`, and the `account_list_key_account` / `family_account_name` resolution sits BELOW that `continue`. `is_sharing_key` answers true only for the three sharing features (`RI_SHARING`, `CREDIT_SHARING`, `CREDIT_LEVEL_SHARING`) and for a `credit/{creditId}/{option}` key whose option is not `status`, so for `BILLING_ALERTS` — the one remaining updatable feature — every key, including one whose last segment is a 12-digit account id, skips the resolution entirely.

Consequence: `UpdateBillingPreferences(feature="BILLING_ALERTS", billingPreferencesPerKey=[{key:"alerts/999999999999", value:"ENABLED"}])` answers 200 and persists a preference key naming an account that is in no consolidated billing family this store holds, and `GetBillingPreferences` then reports that entry with `accountId`/`accountName` absent (because `account_of_key` can only report an account the family holds). The identical key shape under `RI_SHARING` or `CREDIT_LEVEL_SHARING` is refused 403 with "Account 999999999999 is not a member of this consolidated billing family". One of the two sides has to move: either the resolution runs for every key that names a resource, as the module doc and the credit-key suite both state the rule, or the doc must scope it to sharing keys. The inline comment above the loop explains only why the MANAGEMENT-ACCOUNT check is scoped to sharing keys (the Smithy sentence "Sharing keys ... may only be set by the management account of a consolidated billing family"); nothing explains why the family-membership check for a named account inherits that scope.

## Evidence

The rule, stated unconditionally in the handler's own module doc — services/service-com-amazonaws-billing-awsbilling/src/ops/update_billing_preferences.rs:12-16:
```
//! A key that NAMES a resource is resolved against the store that holds it: an
//! account-list key's account against the consolidated billing family, and a
//! `credit/{creditId}/{option}` key's credit against `state::credits`. A key
//! naming a resource the family does not hold is refused 403
//! `AccessDeniedException` and stores nothing.
```
The code that breaks it — same file:299-323 (the account resolution is below the `continue`):
```
        let caller = crate::state::caller_account_id();
        for preference in &input.billing_preferences_per_key {
            if !crate::state::is_sharing_key(feature, &preference.key) {
                continue;
            }
            ...
            if let Some(account) = crate::state::account_list_key_account(&preference.key) {
                if state.family_account_name(account).is_none() {
                    return Err(access_denied(format!(
                        "Account {account} is not a member of this consolidated billing family"
                    )));
                }
            }
        }
```
What `is_sharing_key` admits — services/service-com-amazonaws-billing-awsbilling/src/state.rs:1396-1402:
```
pub fn is_sharing_key(feature: &str, key: &str) -> bool {
    if SHARING_FEATURES.contains(&feature) {
        return true;
    }
    feature == CREDIT_PREFERENCE_OPTIONS_FEATURE
        && credit_key_parts(key).is_some_and(|(_, option)| option != CREDIT_STATUS_OPTION)
}
```
with `SHARING_FEATURES` = `["RI_SHARING", "CREDIT_SHARING", "CREDIT_LEVEL_SHARING"]` (state.rs:297), so `BILLING_ALERTS` never reaches the resolution. The key-shape reader itself is feature-agnostic — state.rs:1412-1415:
```
pub fn account_list_key_account(key: &str) -> Option<&str> {
    let last = key.split('/').next_back()?;
    is_account_id(last).then_some(last)
}
```
The same rule, stated as a generalisation over BOTH key kinds by the credit-key suite — services/service-com-amazonaws-billing-awsbilling/tests/all/credit_preference_keys.rs:6-14: "One rule: a preference key that NAMES a resource is honoured only when that resource is one the caller's consolidated billing family holds and the caller may speak for. So the `{creditId}` of a `credit/{creditId}/{option}` key is resolved against the credits this crate stores ... exactly as an account-list key's trailing account is resolved against the family ... a credit the family does not hold or the caller does not speak for is refused 403 `AccessDeniedException`, and either way nothing is written."

Test coverage: the current acceptance IS pinned — services/service-com-amazonaws-billing-awsbilling/tests/all/billing_preferences.rs:455-468 sends `("BILLING_ALERTS", "alerts/999999999999", None)` through `update(...)` and asserts `status == 200`, under a comment whose stated reason covers only the REPORTING side ("A 12-digit id that is in no family this emulator holds: neither member is reported, so the two never disagree"), not the acceptance of the write. The refusal path is pinned only for the sharing features and for credit keys (tests/all/credit_preference_keys.rs).

## Steps to Reproduce

1. Boot the emulator with a billing awsJson1_0 client (the harness in tests/all/billing_preferences.rs::emulator) and grant BILLING_ALERTS access, which the default state leaves closed (`ComAmazonawsBillingAwsbillingState::grant_feature_access`).
2. Call UpdateBillingPreferences with {"feature":"BILLING_ALERTS","billingPreferencesPerKey":[{"key":"alerts/999999999999","value":"ENABLED"}]} — a 12-digit trailing segment that is in no consolidated billing family this store holds.
3. The call answers 200 and the key is persisted; GetBillingPreferences(features=["BILLING_ALERTS"]) reports the entry with accountId/accountName absent.
4. For contrast, send the same key shape under a sharing feature: UpdateBillingPreferences with {"feature":"CREDIT_LEVEL_SHARING","billingPreferencesPerKey":[{"key":"alerts/999999999999","value":"ENABLED"}]} — refused 403 AccessDeniedException "Account 999999999999 is not a member of this consolidated billing family", and nothing is stored.
