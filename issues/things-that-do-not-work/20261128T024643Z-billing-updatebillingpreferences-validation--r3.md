Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 9448a3fd4343085df79373d9cc834f6f5c8e5bf1

# BENEFIT_LEVEL_SHARING is treated as a non-sharing feature with no key rules, so a member account may set its sharing keys and mixed-benefit keys are stored

_Filed: 2026-11-28T02:46:43Z_
_Operation: UpdateBillingPreferences | Class: validation_

> _Note: this issue was identified against the code base at commit `9448a3fd4343085df79373d9cc834f6f5c8e5bf1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model's `UpdateBillingPreferences` documentation lists FOUR sharing features that only the consolidated-billing-family management account may set, and documents a key form and a same-benefit rule for `BENEFIT_LEVEL_SHARING`. This crate implements neither for that feature:

1. `crate::state::SHARING_FEATURES` omits `BENEFIT_LEVEL_SHARING`, and its own doc comment MISQUOTES the model sentence with that value removed. `is_sharing_key("BENEFIT_LEVEL_SHARING", key)` therefore answers `false` (the feature is not in `SHARING_FEATURES`, and the only other branch is `CREDIT_PREFERENCE_OPTIONS`), so the management-account gate in the handler never fires for it. A caller that is NOT the family's payer account is answered 200 and the preference is stored, where the model requires the management account. The gate is reachable: the crate's own suite sets `billing_family_management_account_id` to another account (tests/all/credit_preference_keys.rs:403) and pins exactly this 403 for the `CREDIT_PREFERENCE_OPTIONS` sibling. `BENEFIT_LEVEL_SHARING` is not behind the access-activation gate (`ACCESS_ACTIVATION_FEATURES` at state.rs:339-340 holds only BILLING_ALERTS/RI_SHARING/RI_SHARING_HISTORY), so the request reaches the sharing gate rather than being refused 403 earlier.

2. The documented per-feature key rule "For `BENEFIT_LEVEL_SHARING`, all keys must reference the same Reserved Instance or Savings Plan" is never applied. The handler implements only the `CREDIT_PREFERENCE_OPTIONS` analogue ("all keys must reference the same `creditId`"), under a comment asserting that "the model documents a key form for exactly one feature" — which the quoted model text contradicts, since it documents `benefit/{ri|sp}/{benefitId}/sharing/...` key forms and `open-sharing` / `costcategory-prioritized/{costCategoryArn}` group keys as well. One request naming `benefit/ri/A/sharing/mode/prioritized` and `benefit/ri/B/sharing/mode/prioritized` is accepted and both keys are stored.

3. Because no `BENEFIT_LEVEL_SHARING` key form is parsed, the only key shaping the handler applies to it is the generic `account_list_key_account` heuristic ("the key's last `/`-separated segment when it is a 12-digit account id"). A documented benefit key whose trailing segment happens to be 12 digits — e.g. `benefit/sp/abc/sharing/ruleName/123456789012`, where the segment names a Cost Category rule, not an account — is refused 403 "Account 123456789012 is not a member of this consolidated billing family".

## Evidence

Model (smithy_query query="doc" shape="UpdateBillingPreferences"): "To set sharing keys (RI_SHARING, CREDIT_SHARING, CREDIT_LEVEL_SHARING, BENEFIT_LEVEL_SHARING, and sharing keys under CREDIT_PREFERENCE_OPTIONS), you must use the management account of a consolidated billing family."

Crate side — services/service-com-amazonaws-billing-awsbilling/src/state.rs:342-348:
```
/// The `BillingFeature` values whose preferences are *sharing* preferences.
///
/// From the `UpdateBillingPreferences` doc: "Sharing keys (`RI_SHARING`,
/// `CREDIT_SHARING`, `CREDIT_LEVEL_SHARING`, and sharing keys under
/// `CREDIT_PREFERENCE_OPTIONS`) may only be set by the management account of a
/// consolidated billing family."
pub const SHARING_FEATURES: &[&str] = &["RI_SHARING", "CREDIT_SHARING", "CREDIT_LEVEL_SHARING"];
```
(the quoted sentence has `BENEFIT_LEVEL_SHARING` dropped from the model's list, and so does the constant)

state.rs:1475-1481 — the only two ways a key can be a sharing key:
```
pub fn is_sharing_key(feature: &str, key: &str) -> bool {
    if SHARING_FEATURES.contains(&feature) {
        return true;
    }
    feature == CREDIT_PREFERENCE_OPTIONS_FEATURE
        && credit_key_parts(key).is_some_and(|(_, option)| option != CREDIT_STATUS_OPTION)
}
```

src/ops/update_billing_preferences.rs:319-327 — the gate that therefore never fires for BENEFIT_LEVEL_SHARING:
```
            if crate::state::is_sharing_key(feature, &preference.key)
                && !state.is_billing_family_management_account(caller)
            {
                return Err(access_denied(format!(
                    "Account {caller} is not the management account of a consolidated billing \
                     family, so it cannot set the sharing preference {}",
                    preference.key
                )));
            }
```

Model (smithy_query query="doc" shape="UpdateBillingPreferencesRequest" member="billingPreferencesPerKey"): "All keys in a single request must be valid for the specified feature and must not be duplicated. For CREDIT_PREFERENCE_OPTIONS, all keys must reference the same creditId." … "For BENEFIT_LEVEL_SHARING, all keys must reference the same Reserved Instance or Savings Plan. To share it with the accounts in one Cost Category rule, send these three keys with the value ENABLED: benefit/{ri|sp}/{benefitId}/sharing/costCategoryArn/{costCategoryArn} …"

vs src/ops/update_billing_preferences.rs:253-258:
```
        // "All keys in a single request must be valid for the specified
        // feature": the model documents a key form for exactly one feature, so
        // that is the one checked here. "For CREDIT_PREFERENCE_OPTIONS, all keys
        // must reference the same creditId."
        let mut named_credit: Option<&str> = None;
        if feature == crate::state::CREDIT_PREFERENCE_OPTIONS_FEATURE {
```

state.rs:1491-1494 — the only key shaping that then applies to a BENEFIT_LEVEL_SHARING key:
```
pub fn account_list_key_account(key: &str) -> Option<&str> {
    let last = key.split('/').next_back()?;
    is_account_id(last).then_some(last)
}
```

Reachability of the gate, state.rs:939-943 (BENEFIT_LEVEL_SHARING is granted access by default, so the request is not refused earlier):
```
            feature_access: BILLING_FEATURE_VALUES
                .iter()
                .filter(|feature| !ACCESS_ACTIVATION_FEATURES.contains(*feature))
```

Test coverage: tests/all/credit_preference_keys.rs:392-438 (`a_member_account_sets_only_the_credits_it_owns`) sets `billing_family_management_account_id` to a non-caller account and pins the 403 for the CREDIT_PREFERENCE_OPTIONS sharing keys; no test in tests/all/ mentions BENEFIT_LEVEL_SHARING or a `benefit/` key at all (grep for `BENEFIT_LEVEL_SHARING|benefit/` matches only OPERATIONS.md, the enum value list in ops/types/shared_types.rs:186 and the raw model).

## Steps to Reproduce

1. Start the emulator with this service mounted and make the caller NOT the family payer, the way tests/all/credit_preference_keys.rs:403 does: `engine.state::<ComAmazonawsBillingAwsbillingState>().lock().billing_family_management_account_id = "111111111111".to_string();`
2. POST / with `X-Amz-Target: AWSBilling.UpdateBillingPreferences` and body `{"feature":"BENEFIT_LEVEL_SHARING","billingPreferencesPerKey":[{"key":"benefit/ri/abc/sharing/mode/prioritized","value":"ENABLED"}]}`
3. Observe HTTP 200 and the key stored (readable through GetBillingPreferences with features=["BENEFIT_LEVEL_SHARING"]) — the model requires the management account of a consolidated billing family for this sharing key, which is the 403 the same handler answers for `credit/{creditId}/sharing/...` under the same state
4. With the default state (caller IS the payer), POST the same target with `{"feature":"BENEFIT_LEVEL_SHARING","billingPreferencesPerKey":[{"key":"benefit/ri/AAA/sharing/mode/prioritized","value":"ENABLED"},{"key":"benefit/ri/BBB/sharing/mode/prioritized","value":"ENABLED"}]}`
5. Observe HTTP 200: two different benefitIds in one request are accepted, although "For BENEFIT_LEVEL_SHARING, all keys must reference the same Reserved Instance or Savings Plan" — the same-resource rule the handler does enforce for CREDIT_PREFERENCE_OPTIONS
6. POST `{"feature":"BENEFIT_LEVEL_SHARING","billingPreferencesPerKey":[{"key":"benefit/sp/abc/sharing/ruleName/123456789012","value":"ENABLED"}]}` and observe 403 "Account 123456789012 is not a member of this consolidated billing family" — the trailing segment is read as an account id although the documented key form names a Cost Category rule there
