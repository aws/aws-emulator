Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cost-optimization-hub.preferences-member-account-discount-visibility-roundtrip cost-optimization-hub.update-preferences-invalid-discount-visibility-rejected
Issue-Shape-Id: com.amazonaws.costoptimizationhub#CostOptimizationHubService
Issue-Key: cost-optimization-hub.preferences-member-account-discount-visibility-roundtrip|_client_error#13|AssertionError|raise AssertionError(
Issue-Key: cost-optimization-hub.update-preferences-invalid-discount-visibility-rejected|_error#19|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 7b33aef958ce687ae1a941d30fa1d2d6a90fe257

# UpdatePreferences accepts memberAccountDiscountVisibility (and GetPreferences reports it) for a member account instead of refusing with the management-account-only ValidationException

> _Note: this issue was identified against the code base at commit `7b33aef958ce687ae1a941d30fa1d2d6a90fe257`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePreferences

## Description
Cause 1 of 1: the emulator treats the single account it presents as the owner of the organization-wide `memberAccountDiscountVisibility` preference, so every write of that member is ACCEPTED and every read reports it, where the recorded account is a member account and AWS refuses the write and omits the member from the read. Failing checks: cost-optimization-hub.preferences-member-account-discount-visibility-roundtrip, cost-optimization-hub.update-preferences-invalid-discount-visibility-rejected.

AWS: `memberAccountDiscountVisibility` is an organization-level preference; only the organization's management account may set it. For a caller that is a member account, `UpdatePreferences` with an IN-ENUM value (`All` or `None`) is refused with the modelled ValidationException — HTTP 400, error code `ValidationException`, message exactly `Only management accounts can update member account discount visibility.` (the whole message is fixed text for this error class; it carries no request- or state-derived values). The rejected call has no effect, and `GetPreferences` for such an account does not report the `memberAccountDiscountVisibility` member at all (it is absent from the response, not reported as a value). Smithy (com.amazonaws.costoptimizationhub): `UpdatePreferences` input/output member `memberAccountDiscountVisibility` targets enum `MemberAccountDiscountVisibility` with values [All, None]; the operation declares `ValidationException` (httpError 400) whose shape carries required `message`, optional `reason`, and optional `fields` (list of `ValidationExceptionDetail{fieldName, message}`). The model states nothing about which account may set the member — recorded behavior only for the management-account restriction and for the member's absence from a member account's `GetPreferences`.

Emulator: both calls return HTTP 200 with the member accepted and echoed. `update_preferences(memberAccountDiscountVisibility='None')` answered 200 with `memberAccountDiscountVisibility: 'None'` alongside savingsEstimationMode/preferredCommitment, and `update_preferences(memberAccountDiscountVisibility='All')` answered 200 with `memberAccountDiscountVisibility: 'All'`. No error code and no message are produced at all for the class, and the stored value then surfaces on the subsequent `GetPreferences`, so the field is both settable and reported where AWS reports nothing. The out-of-enum rejection (`Everyone` -> ValidationException `Input validation failed. Please check the field list for more details.` with fields naming `memberAccountDiscountVisibility`) does work; only the in-enum management-account gate is missing, so the two rejection classes are not distinguishable because one of them never happens.

Why: a client that relies on AWS's behavior sees a member-account write of an organization preference succeed. Code that depends on the refusal — a tenant-isolation test, a least-privilege check, an error-handling branch keyed on `ValidationException` with that message, or logic that treats the member's presence in `GetPreferences` as proof the caller is the management account — takes the wrong branch and silently records an organization-wide setting from an account AWS would never let write it. Negative-path coverage built against the emulator passes while the same code is refused in production.

Fix: the ownership decision in the crate's state, and the gate the handler asks it. `owns_member_account_discount_visibility` resolves the owner from `organization_management_account_id`, which the store's initial value leaves unset, and the owner then falls back to the presented account — so the caller is always the owner and the gate in the handler (which already constructs the correct refusal text) is never reached. The store must model the presented account the way the recordings do, as a MEMBER account of an organization another account manages, so the owner is not the caller; `effective_preferences` then already omits the member from reads. The refusal itself belongs where it is, in the `UpdatePreferences` handler.

Traces: cost-optimization-hub.preferences-member-account-discount-visibility-roundtrip (expects ValidationException on the write and the unchanged field on re-read; got 200 and the new value), cost-optimization-hub.update-preferences-invalid-discount-visibility-rejected (expects the in-enum write refused with a message distinct from the field-validation summary, and the member absent from the following GetPreferences; got 200 and the new value). Relevant code: services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/update_preferences.rs lines 201-204 (the gate) and 275-289 (the refusal), services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/state.rs lines 1068-1080 (`discount_visibility_owner` / `owns_member_account_discount_visibility`), 1141 (`organization_management_account_id: None`) and 1166-1177 (`effective_preferences`).

## Evidence
Check cost-optimization-hub.preferences-member-account-discount-visibility-roundtrip:
  expected: ClientError, Code='ValidationException', HTTP 400, Message='Only management accounts can update member account discount visibility.', and GetPreferences unchanged.
  actual: HTTP 200, body {'savingsEstimationMode': 'BeforeDiscounts', 'memberAccountDiscountVisibility': 'None', 'preferredCommitment': {'term': 'ThreeYears', 'paymentOption': 'AllUpfront'}} — AssertionError: "real AWS rejects this call for a non-management account, but it SUCCEEDED".

Check cost-optimization-hub.update-preferences-invalid-discount-visibility-rejected:
  expected: the in-enum write memberAccountDiscountVisibility='All' refused with Code='ValidationException' and Message='Only management accounts can update member account discount visibility.' (distinct from the out-of-enum message 'Input validation failed. Please check the field list for more details.'), then 'memberAccountDiscountVisibility' absent from GetPreferences.
  actual: HTTP 200, body {'savingsEstimationMode': 'BeforeDiscounts', 'memberAccountDiscountVisibility': 'All', 'preferredCommitment': {'term': 'OneYear', 'paymentOption': 'PartialUpfront'}} — AssertionError: "expected the call to be rejected, but it SUCCEEDED". The emulator never emits the management-account refusal, so the final GetPreferences assertion is unreachable.

Fixed text vs variable parts: the message 'Only management accounts can update member account discount visibility.' is entirely fixed for this error class. Values in the bodies above (account-specific preference values, the enum value submitted, timestamps, content-length) are per-run and are not part of the requirement.

## Steps to Reproduce
1. Boot the emulator and create a cost-optimization-hub client pinned to us-east-1.
2. Ensure enrollment: list_enrollment_statuses(); if the status is not 'Active', call update_enrollment_status(status='Active') and poll until Active.
3. Positive control: update_preferences(savingsEstimationMode='BeforeDiscounts') — succeeds and echoes the mode (this works today).
4. Call update_preferences(memberAccountDiscountVisibility='None') (or 'All').
   Expected: ClientError, HTTP 400, Code 'ValidationException', Message 'Only management accounts can update member account discount visibility.'
   Actual: HTTP 200 with memberAccountDiscountVisibility echoed as the submitted value.
5. Call get_preferences().
   Expected: no 'memberAccountDiscountVisibility' key in the response.
   Actual: the key is present, carrying the value step 4 stored.
6. Replay the two checks in trace_ids to confirm.
