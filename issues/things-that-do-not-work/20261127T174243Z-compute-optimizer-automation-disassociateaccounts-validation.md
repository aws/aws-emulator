Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: 3a5c87ab4fc6926b6c328f1d815f5ccea61a3857

# DisassociateAccounts accepts an account an organization enrollment configuration applies to, which the operation documents as rejected — and then reports it "successfully disassociated" while the management account can still automate in it

_Filed: 2026-11-27T17:42:43Z_
_Operation: DisassociateAccounts | Class: validation_

> _Note: this issue was identified against the code base at commit `3a5c87ab4fc6926b6c328f1d815f5ccea61a3857`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DisassociateAccounts` documents the same rejection its paired operation does: a request naming an account that an organization enrollment configuration applies to must be refused. The handler never tests that. It validates only the `accountIds` size/duplication rule, passes the caller's enrollment gate, and calls `disassociate_account` for every named id, which rewrites the account's own `Direct` rule mode to `NoneAllowed`.

Under an applied `AnyAllowed` configuration that write changes nothing that matters, because the configuration takes precedence in `effective_rule_mode_for_account`. The response is then actively wrong in the dangerous direction: `accountIds` is built from `!guard.is_associated(..)`, which reads only the `Direct` setting, so the account IS reported under "The IDs of the member accounts that were successfully disassociated" — while `may_initiate_in`, which resolves the EFFECTIVE mode, still answers `true`. So after a 200 that claims the account was disassociated, `StartAutomationEvent` and `RollbackAutomationEvent` still succeed for that account's resources, `ListRecommendedActions` still reports its actions, and `CreateAutomationRule` may still scope an organization rule to it — the exact opposite of what the operation's own documentation says a disassociation means ("Once disassociated, organization rules no longer apply to the member account, and the management account (or delegated administrator) can't create automation rules for that account").

As on the association side, the predicate the refusal needs is already in the crate (`EffectiveRuleMode::enabled_by_configuration()`, used by `ListOrganizationalUnits` and `GetEnrollmentConfiguration`) and the handler's own comment asserts this emulator has no organization directory to disagree with a request, which `PutOrganizationEnrollmentConfiguration` disproves by reading one. The scaffolded doc header in the handler file again omits the rejection sentence.

## Evidence

The modelled rule, verbatim — `smithy_query(query="doc",shape="DisassociateAccounts")`:

> "Disassociates member accounts from your organization's management account, removing centralized automation capabilities. Once disassociated, organization rules no longer apply to the member account, and the management account (or delegated administrator) can't create automation rules for that account. **Requests that include an account to which an organization enrollment configuration applies are rejected.**"

The handler, which judges only the list rule and then applies the removal to every named id:
`src/ops/disassociate_accounts.rs:191-200`:
```
if let Some(message) = crate::state::account_ids_violation(&input.account_ids) {
    return Err(
        DisassociateAccountsErrorModeled::InvalidParameterValueException(
```
`src/ops/disassociate_accounts.rs:239-242`:
```
let disassociated_at = crate::state::now();
for account_id in &requested {
    guard.disassociate_account(account_id, disassociated_at);
}
```

The response that claims success from the DIRECT setting only:
`src/ops/disassociate_accounts.rs:277-280`:
```
let account_ids: Vec<String> = requested
    .into_iter()
    .filter(|account_id| !guard.is_associated(account_id))
    .collect();
```
`src/state.rs:3693-3696`:
```
pub fn is_associated(&self, account_id: &str) -> bool {
    self.enrollment(account_id).reported_organization_rule_mode()
        == ORGANIZATION_RULE_MODE_ANY_ALLOWED
}
```
…against the gate every operation that reaches INTO the account consults, which resolves the configuration first and therefore still permits the reach:
`src/state.rs:3723-3729`:
```
pub fn may_initiate_in(&self, hierarchy: &OrganizationHierarchy, account_id: &str) -> bool {
    account_id == caller_account_id()
        || self
            .effective_rule_mode_for_account(hierarchy, account_id)
            .mode
            == ORGANIZATION_RULE_MODE_ANY_ALLOWED
}
```
`src/state/organization_enrollment.rs:603-611` (the configuration is resolved as `Applied`, ahead of anything the disassociation wrote):
```
if let Some(stored) = self.organization_enrollment_configuration(account_id) {
    return EffectiveRuleMode {
        mode: stored.organization_rule_mode.clone(),
        source: OrganizationRuleModeSource {
            source_type: RULE_MODE_SOURCE_APPLIED.to_string(),
```

The handler comment claiming the service cannot disagree with the request:
`src/ops/disassociate_accounts.rs:284-290`:
```
// `errors` is "Any errors that occurred during the disassociation
// process" — one entry per account the service could not
// disassociate. This emulator holds no organization-membership
// directory to disagree with the request, and every id reaching
// here matched `AccountId`, so nothing fails per account and the
// list is empty.
errors: Some(Vec::new()),
```
…contradicted by `src/ops/put_organization_enrollment_configuration.rs:215-220`, which reads that very directory and refuses a request from it:
```
let hierarchy = crate::state::OrganizationHierarchy::read(&*self.engine);
if hierarchy.is_management_account(&target_id) {
```

The already-written predicate the two association handlers skip:
`src/state/organization_enrollment.rs:177-180`:
```
pub fn enabled_by_configuration(&self) -> bool {
    self.source.source_type == RULE_MODE_SOURCE_APPLIED
        || self.source.source_type == RULE_MODE_SOURCE_INHERITED
}
```
called at `src/state/organization_enrollment.rs:701` and `:737-739`.

The scaffolded doc header that omits the rule: `src/ops/disassociate_accounts.rs:56-60` renders "...cannot create Automation rules for that account. Only the management account or a delegated administrator can perform this action." with the rejection sentence missing.

Test coverage: nothing pins the accepted behaviour. `tests/all/organization_enrollment_configurations.rs:406-435` exercises only `DeleteOrganizationEnrollmentConfiguration` after a put; `tests/all/account_disassociation.rs` never calls `PutOrganizationEnrollmentConfiguration` (grep for it in `tests/` matches only `organization_enrollment_configurations.rs`). So no test sends `DisassociateAccounts` for an account a configuration applies to.

## Steps to Reproduce

1. UpdateEnrollmentConfiguration {"status":"Active"} so the caller's enrollment gate is open.
2. AssociateAccounts {"accountIds":["111111111111"]} — the member's analysed inventory is recorded.
3. PutOrganizationEnrollmentConfiguration {"target":{"accountId":"111111111111"},"organizationRuleMode":"AnyAllowed"} — a configuration now applies to the account.
4. DisassociateAccounts {"accountIds":["111111111111"]} — expected: refused, because the model says "Requests that include an account to which an organization enrollment configuration applies are rejected." Actual: HTTP 200 with accountIds ["111111111111"], i.e. reported as successfully disassociated.
5. ListRecommendedActions {"filters":[{"name":"AccountId","values":["111111111111"]}]} — the member's actions are STILL reported, and StartAutomationEvent for one of them still succeeds, so the management account still reaches into an account the previous response said it had disassociated.
6. ListAccounts — the same account still reports organizationRuleMode "AnyAllowed" with organizationRuleModeSource.sourceType "Applied".
