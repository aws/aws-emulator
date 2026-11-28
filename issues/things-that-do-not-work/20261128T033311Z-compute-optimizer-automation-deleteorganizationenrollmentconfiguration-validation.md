Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2f560ffaae8c854f17058ad67df2ccfdef0f2cf0

# A management-account target is accepted with 200, although the target union documents that the management account cannot be specified and the sibling put refuses it

_Filed: 2026-11-28T03:33:11Z_
_Operation: DeleteOrganizationEnrollmentConfiguration | Class: validation_

> _Note: this issue was identified against the code base at commit `2f560ffaae8c854f17058ad67df2ccfdef0f2cf0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteOrganizationEnrollmentConfiguration$target` is the same `OrganizationTarget` union `PutOrganizationEnrollmentConfiguration$target` is, and that union documents that the management account may not be named as an account target. The put handler enforces it: it reads the Organizations hierarchy and refuses a target the organization names as its management account with the modelled 400 `InvalidParameterValueException`, worded by `crate::state::management_account_target_message`.

The delete handler reads the target id and goes straight to the client-token ledger — it never reads the hierarchy and never applies the check — so `DeleteOrganizationEnrollmentConfiguration {"target":{"accountId":<management account>}, …}` is answered 200 and silently consumes the request's client token, instead of being refused with the error the operation declares. The helper the refusal needs (`OrganizationHierarchy::is_management_account`) and the message (`management_account_target_message`) already exist in the crate and are already called by the sibling handler one file away.

## Evidence

The model text on the shared union (`smithy_query(query="shape",shape="OrganizationTarget")`):
- union doc: "An account, organizational unit (OU), or organization root to which an organization enrollment configuration applies. **The management account can't be specified as an account target.**"
- `accountId` member doc: "The ID of the member account to target. **You can't specify the management account.**"

The sibling handler that applies it — `src/ops/put_organization_enrollment_configuration.rs:210-220`:
```
// "Organization enrollment configurations can't be created for the
// management account." …
let hierarchy = crate::state::OrganizationHierarchy::read(&*self.engine);
if hierarchy.is_management_account(&target_id) {
    return Err(invalid_parameter(
        crate::state::management_account_target_message(&target_id),
    ));
}
```

The delete handler, which needs the same check and does not make it — `src/ops/delete_organization_enrollment_configuration.rs:190-204`:
```
let target_id = crate::state::target_id_of(&input.target).to_string();

// The request as THIS handler reads it, which is what its client token
// ties …
let configuration_types = crate::state::canonical_membership(&input.configuration_types);
let request_fingerprint = crate::state::request_fingerprint(&input, |probe| {
    probe.client_token = None;
    probe.configuration_types = configuration_types.clone();
});
```
No `OrganizationHierarchy::read` and no `is_management_account` appears anywhere in this file (`grep is_management_account` matches only `src/state/organization_enrollment.rs` and `src/ops/put_organization_enrollment_configuration.rs:216`).

Test coverage: `core/router/tests/all/compute_optimizer_organization_enrollment_hierarchy.rs:612-650` drives the refusal for the PUT only (`a_configuration_cannot_be_created_for_the_management_account`); `grep -n 'management' ` over that file shows `delete_on_node` (line 346) is only ever called with OU/root ids (lines 575, 596). No test pins the delete's acceptance of a management-account target either way.

## Steps to Reproduce

1. Mount this service beside the Organizations control plane and create an organization, so the management account id is known (as `compute_optimizer_organization_enrollment_hierarchy` does).
2. UpdateEnrollmentConfiguration {"status":"Active"} for the calling account.
3. PutOrganizationEnrollmentConfiguration {"target":{"accountId":"<management account id>"},"organizationRuleMode":"AnyAllowed"} — correctly refused 400 InvalidParameterValueException naming the target.
4. DeleteOrganizationEnrollmentConfiguration {"target":{"accountId":"<the same management account id>"},"configurationTypes":["OrganizationRuleMode"],"clientToken":"t1"} — answered 200 and the token is recorded, although the union documents that the management account can't be specified as an account target.
