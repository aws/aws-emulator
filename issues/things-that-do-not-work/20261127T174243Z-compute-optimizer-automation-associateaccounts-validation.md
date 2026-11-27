Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: 3a5c87ab4fc6926b6c328f1d815f5ccea61a3857

# AssociateAccounts accepts an account an organization enrollment configuration applies to, which the operation documents as rejected — and then reports it as "successfully associated" while every reader says otherwise

_Filed: 2026-11-27T17:42:43Z_
_Operation: AssociateAccounts | Class: validation_

> _Note: this issue was identified against the code base at commit `3a5c87ab4fc6926b6c328f1d815f5ccea61a3857`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateAccounts` documents one rejection beyond the modelled constraints: a request naming an account that an organization enrollment configuration applies to must be refused. The handler never tests that. It validates only the `accountIds` size/duplication rule (`crate::state::account_ids_violation`), passes the caller's enrollment gate, and then writes a `Direct` `AnyAllowed` rule mode onto every named account unconditionally.

Because an organization enrollment configuration "take[s] precedence over settings made by AssociateAccounts and DisassociateAccounts" — which this crate implements in `effective_rule_mode_for_account` (configuration first, then inheritance, then the association's `Direct` mode) — the write the handler performs is shadowed. The result is a hollow success: the response lists the account under `accountIds`, documented as "The IDs of the member accounts that were successfully associated", and `errors` is empty, while `ListAccounts` keeps reporting that account's `organizationRuleMode` as the configuration's value with `organizationRuleModeSource.sourceType = Applied`, and `may_initiate_in` keeps answering from the configuration. The caller is told the association took effect when the service's own resolution says it did not — which is precisely why the documented sentence refuses the request instead of applying it.

The predicate the refusal needs already exists in this crate and is already used by two other read paths: `EffectiveRuleMode::enabled_by_configuration()` is documented as "Is Automation enabled for the accounts this mode covers BY an organization enrollment configuration?" and is true for exactly the `Applied` and `Inherited` source types. `ListOrganizationalUnits` calls it (`organizational_units_in_order`) and `GetEnrollmentConfiguration` calls it (`organization_enrollment_configuration_status`). Only the two association handlers, which are the ones the model says must reject on it, never ask.

The handler's own comment also asserts the opposite of what the crate holds: it says this emulator has no organization directory to disagree with the request, while `PutOrganizationEnrollmentConfiguration` reads exactly such a directory (`OrganizationHierarchy::read`) and refuses a request from it. The scaffolded doc header in the handler file reproduces the operation's documentation with the rejection paragraph missing, which is why the rule was never implemented.

## Evidence

The modelled rule, verbatim — `smithy_query(query="doc",shape="AssociateAccounts")`:

> "Associates one or more member accounts with your organization's management account... When you associate a member account, its organization rule mode is automatically set to \"Any allowed,\"... **Requests that include an account to which an organization enrollment configuration applies are rejected.**"

The handler, which tests nothing of the kind — the only pre-store judgement is the list-size rule, and the association is then applied to every named id:
`src/ops/associate_accounts.rs:194-201`:
```
if let Some(message) = crate::state::account_ids_violation(&input.account_ids) {
    return Err(AssociateAccountsErrorModeled::InvalidParameterValueException(
```
`src/ops/associate_accounts.rs:255-267`:
```
let associated_at = crate::state::now();
for account_id in &requested {
    guard.associate_account(account_id, associated_at);
    ...
    if guard.is_enrolled(account_id) {
        guard.ensure_recommended_action_inventory(account_id);
    }
}
```

The handler's comment claiming the service cannot disagree with the request:
`src/ops/associate_accounts.rs:308-313`:
```
// `errors` is "Any errors that occurred during the association
// process" — one entry per account the service could not associate.
// This emulator holds no organization-membership directory to
// disagree with the request, and every id reaching here matched
// `AccountId`, so nothing fails per account and the list is empty.
errors: Some(Vec::new()),
```

…contradicted by the sibling handler in the SAME crate, which reads an organization directory and refuses a request from it:
`src/ops/put_organization_enrollment_configuration.rs:215-220`:
```
let hierarchy = crate::state::OrganizationHierarchy::read(&*self.engine);
if hierarchy.is_management_account(&target_id) {
    return Err(invalid_parameter(
        crate::state::management_account_target_message(&target_id),
    ));
}
```

The exact predicate "a configuration applies to this target", already written and already used twice in this crate:
`src/state/organization_enrollment.rs:168-180`:
```
/// Is Automation enabled for the accounts this mode covers BY an
/// organization enrollment configuration?
...
pub fn enabled_by_configuration(&self) -> bool {
    self.source.source_type == RULE_MODE_SOURCE_APPLIED
        || self.source.source_type == RULE_MODE_SOURCE_INHERITED
}
```
called at `src/state/organization_enrollment.rs:701` (`ListOrganizationalUnits`' `autoEnrollMemberAccounts`) and at `src/state/organization_enrollment.rs:737-739` (`GetEnrollmentConfiguration`'s `organizationEnrollmentConfigurationStatus`).

Why the accepted write is hollow — the configuration wins over what the association wrote:
`src/state/organization_enrollment.rs:594-597`:
```
/// `AssociateAccounts` and `DisassociateAccounts` write. Configurations come
/// FIRST because they "take precedence over settings made by
/// `AssociateAccounts` and `DisassociateAccounts`"
```
…while the response is built from the DIRECT setting only, so it reports success:
`src/state.rs:3693-3696`:
```
pub fn is_associated(&self, account_id: &str) -> bool {
    self.enrollment(account_id).reported_organization_rule_mode()
        == ORGANIZATION_RULE_MODE_ANY_ALLOWED
}
```
`src/ops/associate_accounts.rs:301-304`:
```
let account_ids: Vec<String> = requested
    .into_iter()
    .filter(|account_id| guard.is_associated(account_id))
    .collect();
```

The scaffolded doc header in the handler file, which drops the rejection paragraph (so the rule was never seen):
`src/ops/associate_accounts.rs:55-63` renders "...the association process automatically enables it. Only the management account or a delegated administrator can perform this action." with no "Requests that include an account to which an organization enrollment configuration applies are rejected." sentence in between.

Test coverage: nothing pins the accepted behaviour. `tests/all/organization_enrollment_configurations.rs:344-404` (`a_configuration_overrides_an_association_and_a_delete_does_not_restore_it`) only ever calls `AssociateAccounts` BEFORE the put (line 348-354 associate, line 365-371 put), and `tests/all/organization_enrollment_configurations.rs:460-492` puts configurations without associating. No test in the crate sends `AssociateAccounts` for an account a configuration already applies to.

## Steps to Reproduce

1. UpdateEnrollmentConfiguration {"status":"Active"} so the caller's enrollment gate is open.
2. PutOrganizationEnrollmentConfiguration {"target":{"accountId":"111111111111"},"organizationRuleMode":"NoneAllowed"} — the account now has a configuration applied to it (ListOrganizationEnrollmentConfigurations reports targetId 111111111111).
3. AssociateAccounts {"accountIds":["111111111111"]} — expected: refused, because the model says "Requests that include an account to which an organization enrollment configuration applies are rejected." Actual: HTTP 200 with accountIds ["111111111111"] and errors [].
4. ListAccounts — the same account reports organizationRuleMode "NoneAllowed" with organizationRuleModeSource.sourceType "Applied", i.e. the association the previous call reported as successful had no effect on the mode any reader resolves.
5. ListRecommendedActions {"filters":[{"name":"AccountId","values":["111111111111"]}]} — empty, and StartAutomationEvent for one of that account's actions is refused 403 AccessDeniedException, confirming the "successful" association granted nothing.
