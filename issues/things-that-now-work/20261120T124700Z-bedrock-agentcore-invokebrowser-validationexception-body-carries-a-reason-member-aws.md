Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-error-double-stop-and-invoke-terminated
Issue-Step: task#4
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-error-double-stop-and-invoke-terminated
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|InvokeBrowser|4xx->4xx|$.reason
Issue-Commit: 233e85f7211280754246a7d02478fa1f321c9471

# InvokeBrowser ValidationException body carries a reason member AWS omits

> _Note: this issue was identified against the code base at commit `233e85f7211280754246a7d02478fa1f321c9471`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeBrowser

## Description
Cause 1 of 1: InvokeBrowser against a browser session that has already been stopped/terminated is refused with the right status, error type and message, but the emulator's ValidationException body carries an extra `reason` member set to the enum value `FieldValidationFailed`, which AWS does not send for this refusal.

AWS: Smithy model services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json — InvokeBrowser (POST /browsers/{browserIdentifier}/sessions/invoke) models errors AccessDeniedException(403), ConflictException-less set, InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400). ValidationException is a structure with members `message` (String), `reason` (enum ValidationExceptionReason: CannotParse | FieldValidationFailed | IdempotentParameterMismatchException | EventInOtherSession | ResourceConflict) and `fieldList`. For the invoke-after-stop refusal the recorded behavior shows AWS serializing the ValidationException body WITHOUT a `reason` key at all: the recording's response has HTTP 400, x-amzn-errortype ValidationException and only the `message` member, whose fixed text the emulator already reproduces (the replay reports no divergence on `$.message`). Recorded behavior only for the omission of `reason`; the enum value list is from the model. Hypothesis: AWS populates `reason` only on refusals that really are per-field input-constraint failures (and then alongside `fieldList`), and leaves it out for a session-state refusal, where no request field is at fault.

Emulator: for the same step the emulator answers 400 with the correct error type and message but adds `"reason": "FieldValidationFailed"` to the body. Replay detail: `$.reason: type_mismatch (expected None, got 'FieldValidationFailed')`; expected status 400, actual status 400, so the status, error code and message all match and the body shape is the only divergence.

Why: `reason` is a modeled enum, so it is exactly the member an SDK consumer branches on to decide what to do with a ValidationException. A client reading `FieldValidationFailed` concludes the request it sent was malformed and will rewrite or re-validate its own input fields, when in truth every field was valid and the only problem is that the browser session is no longer usable — so the correct recovery (start a new browser session and re-issue the action) is never taken. Clients that assert on the exact error body (contract tests, recorded-response comparisons) also see a member AWS never sends.

Fix: the InvokeBrowser handler's error path / ValidationException construction in the bedrock-agentcore data-plane service crate — the session-state refusal must build the ValidationException with `message` only, leaving `reason` (and `fieldList`) unset, and `reason` should be populated only where the refusal really is a per-field input-constraint failure. The `reason` member must not be defaulted to `FieldValidationFailed` by the shared ValidationException serializer for every 400.

Traces: bedrock-agentcore-error-double-stop-and-invoke-terminated#task#4 (the only member of this divergence cluster).

## Evidence
Replay of fixture bedrock-agentcore-error-double-stop-and-invoke-terminated, step task#4, operation InvokeBrowser, verdict mismatch.

Expected (recorded AWS): HTTP 400, x-amzn-errortype: ValidationException, body = ValidationException with `message` only — no `reason` key.
Actual (emulator): HTTP 400, x-amzn-errortype: ValidationException, body additionally contains `"reason": "FieldValidationFailed"`.

Replay detail verbatim:
  status: expected 400, got 400
  $.reason: type_mismatch (expected None, got 'FieldValidationFailed')

No divergence was reported on $.message, so the message's fixed text and its variable parts (the browser identifier and session id taken from the request) already match the recording; only the extra `reason` member diverges.

Model evidence: Smithy ValidationException = {message: String (required), reason: ValidationExceptionReason (enum), fieldList: ValidationExceptionFieldList}; ValidationExceptionReason values = CannotParse, FieldValidationFailed, IdempotentParameterMismatchException, EventInOtherSession, ResourceConflict. InvokeBrowser input = {browserIdentifier (@httpLabel, required), sessionId (@httpHeader x-amzn-browser-session-id, pattern ^[0-9a-zA-Z]{1,40}$, required), action (BrowserAction union, required)} — none of these is at fault in this step, which is what makes FieldValidationFailed the wrong classification.

## Steps to Reproduce
1. Replay wire fixture bedrock-agentcore-error-double-stop-and-invoke-terminated against the emulator (StartBrowserSession, then stop the session, then invoke it).
2. At step task#4 the fixture calls InvokeBrowser (POST /browsers/{browserIdentifier}/sessions/invoke with x-amzn-browser-session-id naming the already-stopped session and a BrowserAction in the body).
3. Compare the 400 response body against the recording: the emulator's ValidationException contains a `reason` member (`FieldValidationFailed`) that the recorded AWS body does not contain.
4. Expected: a ValidationException body carrying only `message`. Actual: the same message plus `"reason": "FieldValidationFailed"`.
