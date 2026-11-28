Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2f560ffaae8c854f17058ad67df2ccfdef0f2cf0

# An account or OU that the organization configuration both includes and excludes is still held to the association standing, contradicting the crate's own "an EXCLUDED id is judged by nothing"

_Filed: 2026-11-28T03:33:11Z_
_Operation: CreateAutomationRule | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2f560ffaae8c854f17058ad67df2ccfdef0f2cf0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state::may_scope_rule_to` is the standing check both rule-write paths run before they touch the store. Its own documentation states that an excluded id is judged by nothing, because the rule does not apply there. The code judges the raw inclusion lists — `scope.account_ids` against `may_initiate_in`, and `scope.organizational_unit_ids` against the node's effective rule mode — without removing the ids the same configuration excludes.

`AccountScope::covers` (the one place the crate decides where a rule actually applies) returns false for an id the configuration excludes, implementing the model's "Exclusions take precedence over inclusions". So for a configuration that names one id in both an inclusion and an exclusion list, the rule provably applies nowhere in that account/OU, yet `may_scope_rule_to` still demands the association (or the `AnyAllowed` node mode) for it and the create is refused 403 `AccessDeniedException` — a refusal over a scope the crate itself says is not in effect. `UpdateAutomationRule` shares the same helper and the same refusal.

## Evidence

The rule the helper states — `src/state.rs:3762-3764`:
"/// is judged by the mode that OU or root\n/// is in right now ([`Self::effective_rule_mode_for_node`]). An EXCLUDED id\n/// is judged by nothing: the rule does not apply there."

The code under it, which never consults the exclusions — `src/state.rs:3785-3798`:
```
let scope = rule_account_scope(configuration);
scope
    .account_ids
    .iter()
    .flatten()
    .all(|account_id| self.may_initiate_in(hierarchy, account_id))
    && scope
        .organizational_unit_ids
        .iter()
        .flatten()
        .all(|node_id| {
            self.effective_rule_mode_for_node(hierarchy, node_id).mode
                == ORGANIZATION_RULE_MODE_ANY_ALLOWED
        })
```

The crate's own decision about where the rule applies, which excludes that very id — `src/state/organization_enrollment.rs:383-390`:
```
if self
    .excluded_account_ids
    .iter()
    .any(|id| id == account_id)
    || hierarchy.account_is_under_any(account_id, &self.excluded_organizational_unit_ids)
{
    return false;
}
```
with its doc at `src/state/organization_enrollment.rs:364-366`: "then the exclusions, which take precedence: \"Exclusions take precedence over inclusions. An account that is in an included OU and is also excluded is excluded.\""

The model text the exclusion members carry (`smithy_query(query="shape",shape="OrganizationConfiguration")`): "Exclusions take precedence over inclusions. An account that is in an included OU and is also excluded is excluded." and `excludedAccountIds`: "An excluded account remains excluded even if it is in an included OU."

The handler that turns the helper's `false` into the 403 — `src/ops/create_automation_rule.rs:301-307`:
```
if !guard.may_scope_rule_to(
    &hierarchy,
    &input.rule_type,
    input.organization_configuration.as_ref(),
) {
    return Err(access_denied());
}
```

Test coverage: `tests/all/organization_rule_scope_gate.rs` drives the gate over included accounts only; `core/router/tests/all/compute_optimizer_organization_enrollment_hierarchy.rs:832-906` exercises exclusions through `AccountScope::covers` (the preview/selection side), not through `may_scope_rule_to`. No case names one id in both an inclusion and an exclusion list on a create, so nothing pins the refusal.

## Steps to Reproduce

1. UpdateEnrollmentConfiguration {"status":"Active"} for the calling account.
2. Do not associate member account 111111111111 (its effective organization rule mode is NoneAllowed).
3. CreateAutomationRule {"name":"r1","ruleType":"OrganizationRule","organizationConfiguration":{"accountIds":["111111111111"],"excludedAccountIds":["111111111111"]},"recommendedActionTypes":["UpgradeEbsVolumeType"],"schedule":{...},"status":"Active"}.
4. The create is refused 403 AccessDeniedException over an account the configuration excludes, so the rule would not have applied there at all — ListAutomationRulePreview with the same scope (which goes through AccountScope::covers) reports nothing of that account, confirming the scope is not in effect.
