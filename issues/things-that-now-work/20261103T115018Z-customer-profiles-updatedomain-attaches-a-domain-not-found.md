Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: customer-profiles.domain-create-update-delete-lifecycle
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles.domain-create-update-delete-lifecycle|run#17|AssertionError|assert mutate_message is None, (
Issue-Commit: 9638f5a216729a574257e8ee9a309b04e31d0ff1

# customer-profiles UpdateDomain attaches a domain-not-found Message where AWS sends the 404 with no Message at all (1 cause)

> _Note: this issue was identified against the code base at commit `9638f5a216729a574257e8ee9a309b04e31d0ff1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDomain

## Description
One net-new behavioral cause: UpdateDomain against a domain that does not exist (never created, or deleted earlier in the run) is refused with the right status and the right error code, but the emulator fills in a Message member that real AWS leaves off entirely for this operation.

Cause 1 of 1: UpdateDomain's ResourceNotFoundException carries an emulator-authored sentence; AWS's UpdateDomain 404 has no Message.

AWS: Smithy: UpdateDomain (PUT /domains/{DomainName}) models ResourceNotFoundException with httpError 404; ResourceNotFoundException has exactly one member, `Message` (target `message`), and it is OPTIONAL (not @required) — so a 404 with the member absent is a well-formed modeled response. Recorded behavior only for the value: the replayed check asserts that for UpdateDomain on an absent domain AWS answers 404 ResourceNotFoundException with NO `Message` member in the body, so `Error.Message` is absent on the client side (botocore reports `.get("Message")` as None). This is operation-specific: the SAME check asserts, and the emulator passes, that GetDomain on the SAME deleted domain DOES carry a Message whose fixed text is 'Domain ' + ' is not found' with the DomainName from the request path interpolated. The divergence is therefore not about wording of a shared sentence but about UpdateDomain emitting a Message where AWS emits none.

Emulator: 404, code ResourceNotFoundException, body Message 'Domain <DomainName> is not found' — the fixed text 'Domain ' + ' is not found' with the requested DomainName as the variable part, produced by the shared helper `crate::state::domain_not_found(&input.domain_name)` that GetDomain legitimately uses. The replay recorded 'Domain sim2real-cp-dom-1a7a45c06bb9 is not found'; that domain name is this run's fill-in of the template, not part of the requirement.

Why: a client that distinguishes AWS's refusals by the presence and shape of Error.Message gets a different answer from the emulator than from AWS on the ordinary 'mutate something that was deleted' path. Harnesses and SDK-level error matchers that assert the UpdateDomain 404 is message-less (or that log/compare the full error envelope) diverge on every such call, and a caller is encouraged to depend on a sentence AWS never sends for this operation — the reverse of the usual defect, where a message is missing.

Fix: the UpdateDomain handler's domain-lookup refusal — it must construct ResourceNotFoundException with `message: None` for this operation rather than reusing the domain-scoped not-found sentence helper that GetDomain and the other domain-scoped reads share. Keep the shared helper for the operations whose AWS 404 does carry a sentence; UpdateDomain needs its own message-less refusal. The comment in the handler asserting the refusal is 'worded as every domain-scoped operation words it' is the incorrect premise.

Related but not covering: open issue 20261103T055557Z-customer-profiles-resource-not-found-and-deleteprofile-success.md collects per-operation not-found WORDING causes (GetDomain, SearchProfiles, CreateProfile, DeleteProfile) from the wire corpus; none of its five causes is UpdateDomain, and none of them is about a Message that must be ABSENT. Open issue 20261102T122410Z-customer-profiles-10-net-new-behavioral-causes-refusal.md likewise covers other refusal messages, not this one.

Traces: behavioral check customer-profiles.domain-create-update-delete-lifecycle.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, customer-profiles 21/45 passed; this check is verdict=mismatch and is the only failing check not already marked ALREADY FILED or OWNED.

Failing check customer-profiles.domain-create-update-delete-lifecycle, at the final assertion:

  AssertionError: expected UpdateDomain on deleted domain to carry no message, got 'Domain sim2real-cp-dom-1a7a45c06bb9 is not found'

The assertions immediately before it PASSED, which isolates the cause:
  - GetDomain on the deleted domain -> code 'ResourceNotFoundException' and message exactly 'Domain <domain> is not found' (matches AWS; asserted and passed).
  - UpdateDomain on the deleted domain -> code 'ResourceNotFoundException' (matches AWS; asserted and passed).
  - UpdateDomain's message -> expected None (member absent), got the sentence above.

Expected vs actual for the diverging call:
  Expected: HTTP 404, Error.Code 'ResourceNotFoundException', Error.Message ABSENT from the body.
  Actual:   HTTP 404, Error.Code 'ResourceNotFoundException', Error.Message = 'Domain ' + <DomainName from the request> + ' is not found'.

Source of the emitted value: services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/update_domain.rs lines 214-220 build ResourceNotFoundException with `message: Some(crate::state::domain_not_found(&input.domain_name))`.

Model check: smithy_query(operation="UpdateDomain") lists ResourceNotFoundException/404 among its errors; smithy_query(shape="ResourceNotFoundException") shows a single, non-required member `Message` targeting `message`, so omitting it is modeled-valid.

## Steps to Reproduce
1. CreateDomain(DomainName=D, DefaultExpirationDays=30).
2. GetDomain(DomainName=D) -> 200, DefaultExpirationDays 30.
3. UpdateDomain(DomainName=D, DefaultExpirationDays=60) -> 200; GetDomain reflects 60.
4. DeleteDomain(DomainName=D).
5. GetDomain(DomainName=D) -> expect 404 ResourceNotFoundException with Message 'Domain D is not found' (the emulator already matches this).
6. UpdateDomain(DomainName=D, DefaultExpirationDays=90) -> expect 404 ResourceNotFoundException with NO Message member in the body (botocore: e.response['Error'].get('Message') is None). Observe instead Message 'Domain D is not found'.

A domain name that was never created at all reaches the same branch in step 6. Re-replay behavioral check customer-profiles.domain-create-update-delete-lifecycle to confirm the fix; it also guards step 5, so the GetDomain sentence must stay as it is.
