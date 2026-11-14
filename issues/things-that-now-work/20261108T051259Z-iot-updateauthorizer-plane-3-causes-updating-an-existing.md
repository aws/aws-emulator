Issue-Source: trace-sweep-divergence
Issue-Trace: iot-authorizer-active-delete-conflict-then-recover
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-authorizer-active-delete-conflict-then-recover iot-authorizer-error-paths iot-custom-authorizer-lifecycle
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|UpdateAuthorizer|2xx->4xx|$.authorizerArn
Issue-Key: iot|UpdateAuthorizer|2xx->4xx|$.authorizerName
Issue-Key: iot|UpdateAuthorizer|2xx->4xx|status
Issue-Key: iot|UpdateAuthorizer|4xx->4xx|$.message
Issue-Commit: 89584cbdc793a0842735dd0ecc1a58bc9d0678bb

# iot UpdateAuthorizer plane: 3 causes — updating an existing authorizer answers 404 and the not-found refusal is emulator-authored

> _Note: this issue was identified against the code base at commit `89584cbdc793a0842735dd0ecc1a58bc9d0678bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAuthorizer

## Description
Three net-new divergences in the iot UpdateAuthorizer handler: an authorizer that still exists in the account cannot be updated (the handler answers ResourceNotFoundException where AWS answers 200 and the updated identity), and when the authorizer genuinely is absent the refusal text is the emulator's own prose rather than AWS's.

Cause 1 of 3: UpdateAuthorizer answers 404 for an authorizer that exists, losing the whole identity payload.
AWS: Smithy UpdateAuthorizer — PUT /authorizer/{authorizerName}, success code 200, output members authorizerName (@length 1..128, @pattern ^[\w=,@-]+$) and authorizerArn (@length 0..2048); ResourceNotFoundException (httpError 404) is modelled only for an authorizerName that names no authorizer in the account. Status transitions of an authorizer (ACTIVE -> INACTIVE) are performed through this operation, so for a live authorizer AWS returns 200 with the authorizer's name and ARN and no body-level error. Recorded behavior: the fixture creates the authorizer, has DeleteAuthorizer refused because the authorizer is ACTIVE, and then calls UpdateAuthorizer with status INACTIVE, which AWS answers 200 with authorizerName and authorizerArn.
Emulator: answers 404 and omits both $.authorizerArn and $.authorizerName (detail: "status: expected 200, got 404; $.authorizerArn: missing; $.authorizerName: missing"). The authorizer is not in the emulator's state at that point because the preceding DeleteAuthorizer was accepted instead of being refused for an ACTIVE authorizer — the delete-gating half of that sequence is already owned by the open issue 20261107T022027Z-iot-registry-and-authorizer-state-10-causes-deletes.md, but that issue does not carry this UpdateAuthorizer step's replay key, so the update-path divergence is unowned and filed here.
Why: the documented recovery path for an ACTIVE authorizer — update it to INACTIVE, then delete it — is unusable. A client walking that sequence gets ResourceNotFoundException for a resource DescribeAuthorizer/ListAuthorizers presented a moment earlier, so it cannot distinguish "the authorizer is gone" from "the update was refused", and it has no authorizerArn to continue with.
Fix: the UpdateAuthorizer handler and the authorizer state it reads — the lookup must resolve any authorizer the account holds and apply status/tokenKeyName/authorizerFunctionArn/enableCachingForHttp to it, returning 200 with authorizerName and authorizerArn, and must reserve ResourceNotFoundException for a name the store genuinely does not hold.
Traces: iot-authorizer-active-delete-conflict-then-recover#task#5.

Cause 2 of 3: the same 404-on-an-existing-authorizer fault on the teardown update path, where only the status is asserted.
AWS: Smithy UpdateAuthorizer — success code 200 for an authorizer present in the account; ResourceNotFoundException (404) only for an absent one. Recorded behavior: the step is a status update issued during teardown against the authorizer the fixture created, which AWS answers 200.
Emulator: answers 404 (detail: "status: expected 200, got 404").
Why: teardown and any idempotent re-drive of a provisioning script fail, because the operation that is supposed to quiesce an authorizer before deleting it reports the authorizer does not exist. A client treats 404 as terminal and abandons the resource, leaving it behind.
Fix: same component as cause 1 — the UpdateAuthorizer handler's authorizer lookup and the state it resolves against.
Traces: iot-authorizer-error-paths#teardown#7.

Cause 3 of 3: the genuine not-found refusal carries emulator-authored wording.
AWS: Smithy UpdateAuthorizer error ResourceNotFoundException, httpError 404, member message. AWS's message for an authorizer that names nothing is the fixed sentence "Authorizer <authorizerName> not found" — fixed text "Authorizer " ... " not found", with the single variable part being the authorizerName copied from the request's URI label. Recorded response: 404 with message "Authorizer sim2real-probe-iot-iot-authorizer-2d9858ab-missing not found".
Emulator: 404 with message "Authorizer <authorizerName> does not exist" — the recorded run shows "Authorizer sim2real-probe-iot-iot-authorizer-2d9858ab-missing does not exist". The status and error class are right; the trailing fixed text is the emulator's own phrase.
Why: the message is part of the contract for this error class. Clients, SDK retry/classification helpers and test assertions that match AWS's "... not found" sentence do not match "... does not exist", so a correctly-handled missing-authorizer case is reported as an unrecognised failure. The open issue 20261107T022027Z-iot-not-found-refusals-10-causes-emulator-authored.md describes the same family of rewording for other iot resources but does not carry this UpdateAuthorizer step's replay key, so this cause is filed here.
Fix: the UpdateAuthorizer handler's not-found refusal construction — emit AWS's fixed "Authorizer <name> not found" text with the requested name interpolated, instead of the locally-worded "does not exist" sentence.
Traces: iot-custom-authorizer-lifecycle#task#9.

## Evidence
Cause 1 — fixture iot-authorizer-active-delete-conflict-then-recover step task#5, UpdateAuthorizer: expected status 200 with $.authorizerArn and $.authorizerName present; actual status 404 with both members missing.
Cause 2 — fixture iot-authorizer-error-paths step teardown#7, UpdateAuthorizer: expected status 200; actual status 404.
Cause 3 — fixture iot-custom-authorizer-lifecycle step task#9, UpdateAuthorizer: status 404 on both sides, but $.message expected AWS's "Authorizer <name> not found" and the emulator answered "Authorizer <name> does not exist".
Model citations: Smithy UpdateAuthorizer http {method PUT, uri /authorizer/{authorizerName}, code 200}; output members authorizerName, authorizerArn; errors include ResourceNotFoundException (httpError 404) with a message member.

## Steps to Reproduce
1. Replay fixture iot-authorizer-active-delete-conflict-then-recover against the emulator and stop at step task#5: CreateAuthorizer (status ACTIVE) -> DeleteAuthorizer (must be refused while ACTIVE) -> UpdateAuthorizer with status INACTIVE. The update answers 404 with no authorizerArn/authorizerName instead of 200.
2. Replay fixture iot-authorizer-error-paths step teardown#7: UpdateAuthorizer against the authorizer the fixture created answers 404 instead of 200.
3. Replay fixture iot-custom-authorizer-lifecycle step task#9: UpdateAuthorizer against an authorizerName that was never created; compare $.message to AWS's "Authorizer <name> not found".
