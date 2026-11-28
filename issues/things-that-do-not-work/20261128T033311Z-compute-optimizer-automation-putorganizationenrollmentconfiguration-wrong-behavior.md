Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2f560ffaae8c854f17058ad67df2ccfdef0f2cf0

# A stored organization enrollment configuration never enables Automation for the member accounts it affects, although the operation documents that it does

_Filed: 2026-11-28T03:33:11Z_
_Operation: PutOrganizationEnrollmentConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2f560ffaae8c854f17058ad67df2ccfdef0f2cf0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation's own model documentation states that storing an organization enrollment configuration ENABLES Automation for the accounts the target covers. The handler stores the configuration and, for every account the target covers, only DROPS that account's direct `organizationRuleMode`; it never sets the account's enrollment `status` to the enrolled status and never records the account's recommended-action inventory.

The crate implements exactly this "enable the feature for the affected accounts" rule on its two sibling enrolment paths: `AssociateAccounts` (via `state::associate_account`, which sets `ENROLLED_STATUS`, followed by `ensure_recommended_action_inventory`) and `UpdateEnrollmentConfiguration` (which calls `ensure_recommended_action_inventory` when the status it stores is the enrolled one). `PutOrganizationEnrollmentConfiguration` is the third path the model documents as enabling Automation, and it is the only one that enables nothing.

The result is a self-contradictory pair of responses for the same organization. `ListOrganizationalUnits` reports `autoEnrollMemberAccounts: All` for a node a configuration applies to — the crate's own constant documents that value as "An organization enrollment configuration applies and enables Automation for affected member accounts, including accounts that join later" — while `ListAccounts` reports an account under that very node with `status: Inactive`, i.e. Automation NOT enabled, and that account still has no recommended-action inventory, so `ListRecommendedActions`, both preview listings and `StartAutomationEvent` have nothing of it to act on even though `may_initiate_in` now admits it (its effective mode is `AnyAllowed`, inherited).

The handler's only write is `guard.put_organization_enrollment_configuration(...)`; the enabling half of the documented sentence is simply not implemented.

## Evidence

The Smithy sentence that states the behaviour, verbatim (`smithy_query(query="doc",shape="PutOrganizationEnrollmentConfiguration")`):
"The organization enrollment configuration sets the organization rule mode on the specified account, organizational unit (OU), or organization root **and enables Automation for affected member accounts** that have opted in to Compute Optimizer, including accounts that join later."

The crate quotes that same sentence and names the affected accounts — `src/state/organization_enrollment.rs:635-638`:
"/// A configuration \"sets the organization rule mode on the specified\n/// account, organizational unit (OU), or organization root and enables\n/// Automation for affected member accounts\", so these are the accounts\n/// affected by it."

…and then states the opposite of the quoted sentence — `src/state/organization_enrollment.rs:469-471`:
"/// untouched, which is what \"Automation stays enabled for affected accounts\"\n/// after a delete means." (preceded by "Each account's enrollment STATUS is")

The write itself only clears the direct mode — `src/state/organization_enrollment.rs:484-490`:
```
for account_id in self.accounts_covered_by(hierarchy, target_id) {
    if let Some(enrollment) = self.enrollments.get_mut(&account_id) {
        if enrollment.organization_rule_mode.take().is_some() {
            enrollment.last_updated_timestamp = Some(at);
        }
    }
}
```

The handler makes no other write — `src/ops/put_organization_enrollment_configuration.rs:256-263`:
```
guard.put_organization_enrollment_configuration(
    &hierarchy,
    &target_id,
    target_type,
    &organization_rule_mode,
    crate::state::now(),
);
guard.record_token(client_token, Self::NAME, &request_fingerprint, None);
```

The sibling path that DOES implement the same sentence — `src/state.rs:3878-3884`:
```
if enrollment.status != ENROLLED_STATUS {
    enrollment.status = ENROLLED_STATUS.to_string();
    enrollment.status_reason = None;
    changed = true;
}
```
and `src/ops/associate_accounts.rs:264-266`:
```
if guard.is_enrolled(account_id) {
    guard.ensure_recommended_action_inventory(account_id);
}
```
and `src/ops/update_enrollment_configuration.rs:271-273`:
```
if input.status == crate::state::ENROLLED_STATUS {
    guard.ensure_recommended_action_inventory(&account_id);
}
```
with the inventory helper's own rule — `src/state.rs:3986-3988`: "Called from the operations that ENROLL an account — never from a read path, so what a reader sees does not depend on who read before it."

The crate's reading of the value it DOES report for the affected node — `src/state/organization_enrollment.rs:98-102`:
"/// The `OrganizationEnrollmentConfigurationStatus` of a member account an\n/// organization enrollment configuration enables Automation for" and `src/state/organization_enrollment.rs:169-180` (`enabled_by_configuration`): "Is Automation enabled for the accounts this mode covers BY an organization enrollment configuration? … because a configuration \"enables Automation for affected member accounts … whether `AnyAllowed` or `NoneAllowed`\"".

Test coverage: `tests/all/organization_enrollment_configurations.rs:316-320` asserts `status` is unchanged by a put — but it seeds BOTH probe accounts as already enrolled (`seed_enrollment(&engine, MEMBER_A, state::ENROLLED_STATUS, …)` at line 254-261), so the assertion holds either way and nothing pins the behaviour for an account that is not enrolled. `core/router/tests/all/compute_optimizer_organization_enrollment_hierarchy.rs:528` states "`NoneAllowed` still enables Automation" and asserts `autoEnrollMemberAccounts == All` for the configured nodes, but asserts no account status under them.

## Steps to Reproduce

1. UpdateEnrollmentConfiguration {"status":"Active"} for the calling (management) account, so the service-wide gate is open.
2. UpdateEnrollmentConfiguration for member account 111111111111 is NOT called, and AssociateAccounts is NOT called: the account is a plain member of the organization, placed under OU ou-abcd-11111111 by the Organizations control plane.
3. PutOrganizationEnrollmentConfiguration {"target":{"organizationalUnitId":"ou-abcd-11111111"},"organizationRuleMode":"AnyAllowed"} — 200, the configuration is stored.
4. ListOrganizationalUnits {} — the ou-abcd-11111111 row reports autoEnrollMemberAccounts "All", i.e. the configuration "enables Automation for affected member accounts".
5. ListAccounts {} — 111111111111 is either absent entirely or (if the store already held an entry) reported with status "Inactive": Automation was not enabled for the affected account.
6. ListRecommendedActions {"filters":[{"name":"AccountId","values":["111111111111"]}]} — empty, because no inventory was recorded for the account the configuration enabled, so an organization rule scoped through that OU can never act on anything.
