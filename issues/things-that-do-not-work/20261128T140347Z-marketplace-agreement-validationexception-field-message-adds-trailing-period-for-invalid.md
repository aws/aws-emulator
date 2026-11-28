Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-agreement-validation-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-agreement-validation-error-paths
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement|ListAgreementCancellationRequests|4xx->4xx|$.fields[name="partyType"].message
Issue-Commit: 6b12489e1ebb5e6d9c766b33956d5bab581b07ea

# ValidationException field message adds trailing period for invalid member value

> _Note: this issue was identified against the code base at commit `6b12489e1ebb5e6d9c766b33956d5bab581b07ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAgreementCancellationRequests

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: the `ValidationExceptionField$message` this service states for a member that carried a value it does not accept is worded with a trailing period the real service does not emit.

AWS: Smithy (services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/smithy-model/...): `ListAgreementCancellationRequests` input member `partyType` is `@required`, targets `PartyType` (string, @length min 1 max 32, pattern ^[A-Za-z]+$) and is documented "Use Proposer to list cancellation requests where you are the seller, or Acceptor to list cancellation requests where you are the buyer"; the operation models `ValidationException` (httpError 400) whose `fields` is a list of `ValidationExceptionField{name (required), message (required)}`. The model does not state the text of that per-field message, so for the wording this is RECORDED BEHAVIOR ONLY: the recorded response for a `partyType` that is a well-formed string outside the admitted party-type set is HTTP 400, error `ValidationException`, with a `fields` entry whose `name` is the offending member path and whose `message` is exactly `Member is invalid` — no trailing period. In that message the complete sentence `Member is invalid` is FIXED text; the only per-request/per-state values in the body are `fields[].name` (the dotted member path taken from the request, here `partyType`) and the envelope's `requestId`, neither of which diverged.

Emulator: for the same step the emulator answers the correct status (400) and the correct `reason`/`fields[].name`, but renders `fields[name="partyType"].message` as `Member is invalid.` — the same sentence with a trailing `.` appended. The replay reports `$.fields[name="partyType"].message: value_mismatch (expected 'Member is invalid', got 'Member is invalid.')`. The emulator keeps two distinct wordings for the two `INVALID_…` classes: `FIELD_INVALID = "Member is invalid."` (used for an unacceptable VALUE, which is this case) and `FIELD_INVALID_REFERENCE = "Member is invalid"` (used for an identifier that resolves to nothing). The recording shows the unacceptable-value class uses the un-punctuated sentence, so the punctuated constant is wrong for that class.

Why: the per-field message is part of the error contract a client sees. A client or SDK test that branches on or asserts the field message text — and any consumer that surfaces it to a user or compares it against AWS's wording — sees a string the real service never returns. Because the wrong constant is the shared unacceptable-value sentence, every operation of this service that refuses a member's value (a missing/invalid modelled constraint caught by the boundary validator as well as a handler-level refusal of an unknown party type, catalog, agreement type, filter name or sort attribute) reports the same wrong text, so the divergence is far wider than the one replayed step.

Fix: the service's validation-error construction — the shared `ValidationExceptionField$message` text for the unacceptable-value refusal class, i.e. the `RefusalKind::Invalid` wording used by both the boundary-validator envelope and the handler-raised envelope. It is one constant, not per-operation logic, so the fix belongs there and not in the `ListAgreementCancellationRequests` handler. Verify the sibling classes (unresolved reference, missing member, missing-for-intent) keep their own recorded wording rather than being collapsed into one string.

Traces: marketplace-agreement-validation-error-paths#task#2 (ListAgreementCancellationRequests, expected 400 / actual 400, field message mismatch).

No open issue covers this: list_existing_issues reports nothing filed for marketplace-agreement.

## Evidence
Fixture marketplace-agreement-validation-error-paths, step task#2, operation ListAgreementCancellationRequests.
Expected status 400, actual status 400 (status and error class agree).
Diverging value — detail from replay: `$.fields[name="partyType"].message: value_mismatch (expected 'Member is invalid', got 'Member is invalid.')`.
  Expected error body member: fields[name="partyType"].message == "Member is invalid"
  Actual   error body member: fields[name="partyType"].message == "Member is invalid."
Fixed vs variable parts of the message: the whole sentence "Member is invalid" is fixed text for this error class; the request-derived value in the body is fields[].name ("partyType"), and requestId is a per-run value (not asserted).
Emulator source carrying the wording: services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/state/failures.rs line 72 (`pub const FIELD_INVALID: &str = "Member is invalid.";`), selected at line 124 by `RefusalKind::Invalid => FIELD_INVALID` in `RefusalKind::field_message()`; the un-punctuated recorded text exists at line 79 (`FIELD_INVALID_REFERENCE = "Member is invalid"`) but is reserved for the unresolved-reference class. The refusal for this step is raised in services/.../src/ops/list_agreement_cancellation_requests.rs line 180-187 via `crate::state::validation_exception("INVALID_PARTY_TYPE", "partyType")`, which routes through `validation_exception_naming` -> `validation_exception_in_kind` (failures.rs lines 309-366) with RefusalKind::Invalid.
Smithy evidence: ValidationException (errorKind client, httpError 400) members requestId/message/reason/fields; ValidationExceptionField members name (required) and message (required); ListAgreementCancellationRequests.partyType required, target PartyType @length(1,32) pattern ^[A-Za-z]+$. The model states no text for ValidationExceptionField$message, so the recorded response is the authority.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture `marketplace-agreement-validation-error-paths`, step `task#2`: call ListAgreementCancellationRequests with a `partyType` that satisfies the modelled constraints (1..32 chars, ^[A-Za-z]+$) but is neither of the admitted party types (e.g. anything other than Proposer/Acceptor).
3. Observe the 400 ValidationException body and read `fields[name="partyType"].message`.
4. Emulator returns "Member is invalid."; the recording expects "Member is invalid".
