Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-jobs-data.start-command-execution-reused-token-with-changed-timeout-rejected iot-jobs-data.start-next-status-details-malformed-key-rejected
Issue-Shape-Id: com.amazonaws.iotjobsdataplane#IotLaserThingJobManagerExternalService
Issue-Key: iot-jobs-data.start-command-execution-reused-token-with-changed-timeout-rejected|run#13|AssertionError|assert (code, status, message) == ("ConflictException", 409, _TOKEN_CONFLICT), (
Issue-Key: iot-jobs-data.start-next-status-details-malformed-key-rejected|run#12|AssertionError|assert got == expected, f"a dotted statusDetails key: expected {expected!r}, got {got!r}"
Issue-Commit: 44393db1f79b587bcbd2de03c9db6137eee45723

# iot-jobs-data: 2 net-new causes — StartCommandExecution's reused-clientToken ConflictException and StartNextPendingJobExecution's statusDetails map-key refusal both carry emulator-authored messages

> _Note: this issue was identified against the code base at commit `44393db1f79b587bcbd2de03c9db6137eee45723`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCommandExecution

## Description
Two request refusals in the iot-jobs-data (IoT Jobs data plane) service return the correct HTTP status and error code but a message body the service never words that way. The error message is part of the contract, so each is a divergence. Shared shape of the fault: these handlers compose their own sentences instead of rendering the service's refusal text (AWS's idempotency-conflict sentence, and AWS's aggregated "<N> validation error detected: ..." constraint template).

Cause 1 of 2 — StartCommandExecution: a clientToken replayed with different parameters is refused 409 ConflictException with the emulator's own sentence.
AWS: Smithy — StartCommandExecution (POST /command-executions) input clientToken -> ClientRequestTokenV2, @idempotencyToken, @length 1..64, @pattern ^[\x21-\x7E]+$; modeled error ConflictException, httpError 409. The member's own documentation states the rule: "If you retry a request with the same token and the same parameters, the request will complete successfully. However, if you retry the request using the same token but different parameters, an HTTP 409 conflict occurs." The recorded behavior for that class is 409 ConflictException with the message "Client token was previously used and hasn't been expired yet." — that sentence is entirely fixed text for this error class (it interpolates no request value, no token, no id, no parameter name).
Emulator: answers the right status and code (409 ConflictException) but the message "clientToken was already used for a request with different parameters" — emulator-authored prose that exists nowhere in the service's vocabulary.
Why: a client or SDK-level error handler that distinguishes "token still live" from other 409 conflicts matches on that sentence, and any operator-facing tooling surfaces text the real service never emits; an idempotency retry loop keyed on the message will not recognise the conflict.
Fix: StartCommandExecution's handler / its clientToken idempotency check in the iot-jobs-data data-plane crate — raise the conflict with the service's fixed sentence rather than a locally composed one.
Traces: check iot-jobs-data.start-command-execution-reused-token-with-changed-timeout-rejected (the check asserts the triple (code, status, message) and only the message diverged).

Cause 2 of 2 — StartNextPendingJobExecution: a statusDetails map key outside the key constraints is refused 400 InvalidRequestException with a hand-written sentence instead of the aggregated map-key constraint message.
AWS: Smithy — StartNextPendingJobExecution (PUT /things/{thingName}/jobs/$next) input statusDetails -> DetailsMap, whose key DetailsKey carries @length 1..128 and @pattern ^[a-zA-Z0-9:_-]+$; modeled error InvalidRequestException, httpError 400. The refusal for a key violating those constraints is the aggregated map-key template: "<N> validation error detected: Value '{<key>=<value>}' at 'statusDetails' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [a-zA-Z0-9:_-]+]". Fixed text: the "validation error detected: Value '" preamble, "' at '", "' failed to satisfy constraint: Map keys must satisfy constraint: " and the bracketed list of all three key clauses in that order, with the pattern rendered UNANCHORED ([a-zA-Z0-9:_-]+), not with the model's ^...$ anchors. Variable parts: the error count, the offending entry rendered as {key=value}, and the member path — all taken from the request.
Emulator: answers 400 InvalidRequestException with "the statusDetails name stage.one is not 1..128 characters matching ^[a-zA-Z0-9:_-]+$" — no validation-error count, the offending entry not rendered as {key=value}, the three key clauses collapsed into one, and the pattern printed with the model's anchors.
Why: callers and test tooling parse these sentences to learn which member and which constraint failed; a caller that copies the quoted regex to pre-validate keys applies an anchored pattern the service does not publish, and the collapsed sentence hides that length and pattern are separate constraints on the key.
Fix: the request-validation / constraint-message layer used by StartNextPendingJobExecution, before any state read — build map-key refusals from the aggregated "Map keys must satisfy constraint: [...]" template and render @pattern unanchored.
Traces: check iot-jobs-data.start-next-status-details-malformed-key-rejected (status 400 and code InvalidRequestException matched; only the message diverged, and the subsequent assertions that the refused start leaves the execution QUEUED at version 1 were not reached).

Similar open issues that do NOT cover these: 20261128T073933Z-iot-jobs-data-validation-refusals-diverge-from-aws.md describes the same family of wording defects from the wire corpus (its Cause 3 is the same StartNextPendingJobExecution map-key message, keyed on a different recorded step) — this behavioral check is not carried by it and the report did not mark it owned, so it is filed here and should be closed together with that issue's Cause 3. 20261127T010512Z-iot-jobs-data-plane-updatejobexecution-error-response--r2.md concerns UpdateJobExecution ordering, and 20261127T010512Z-iot-jobs-data-plane-startnextpendingjobexecution-maintainability.md concerns duplicated write sequencing; neither addresses either message above.

## Evidence
Cause 1 — check iot-jobs-data.start-command-execution-reused-token-with-changed-timeout-rejected:
  StartCommandExecution replayed with the same clientToken but a changed executionTimeoutSeconds.
  expected (code, HTTP status, message) = ('ConflictException', 409, 'Client token was previously used and hasn\'t been expired yet.')
  actual   (code, HTTP status, message) = ('ConflictException', 409, 'clientToken was already used for a request with different parameters')
  The whole expected sentence is fixed text for this error class — it carries no per-run value — so the divergence is the message text itself, not an interpolated id.

Cause 2 — check iot-jobs-data.start-next-status-details-malformed-key-rejected:
  StartNextPendingJobExecution with statusDetails={'stage.one': 'warm'} (a dotted key, illegal under DetailsKey's @pattern).
  expected (code, HTTP status, message) = ('InvalidRequestException', 400, "1 validation error detected: Value '{stage.one=warm}' at 'statusDetails' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [a-zA-Z0-9:_-]+]")
  actual   (code, HTTP status, message) = ('InvalidRequestException', 400, 'the statusDetails name stage.one is not 1..128 characters matching ^[a-zA-Z0-9:_-]+$')
  Fixed text of the expected message: the "<N> validation error detected: Value '" preamble, "' at '", "' failed to satisfy constraint: Map keys must satisfy constraint: ", and the three bracketed key clauses in that order with the unanchored regex. Variable parts filled from the request: the rendered entry {stage.one=warm} and the member path 'statusDetails' (the key/value shown are this check's inputs, not part of the template).

## Steps to Reproduce
1. Boot the emulator from this checkout and run the behavioral suite.
2. Cause 1: create a thing and a command, then call StartCommandExecution with an explicit clientToken; repeat the identical call with the same clientToken but a different executionTimeoutSeconds. The second call is refused 409 ConflictException — inspect the message: it reads 'clientToken was already used for a request with different parameters' instead of 'Client token was previously used and hasn't been expired yet.'
3. Cause 2: create a thing and a job targeting it, wait for its execution to read QUEUED, then call StartNextPendingJobExecution(thingName=<thing>, statusDetails={'stage.one': 'warm'}). The call is refused 400 InvalidRequestException — inspect the message: it is the emulator's single sentence quoting the anchored pattern instead of AWS's aggregated "1 validation error detected: Value '{stage.one=warm}' at 'statusDetails' failed to satisfy constraint: Map keys must satisfy constraint: [...]" message.
4. Re-replay checks iot-jobs-data.start-command-execution-reused-token-with-changed-timeout-rejected and iot-jobs-data.start-next-status-details-malformed-key-rejected to confirm both causes are fixed.
