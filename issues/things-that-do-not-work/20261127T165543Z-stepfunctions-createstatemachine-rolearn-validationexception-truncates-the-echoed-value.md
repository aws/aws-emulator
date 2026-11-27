Issue-Source: trace-sweep-divergence
Issue-Trace: stepfunctions-create-state-machine-role-arn-length-bounds
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: stepfunctions-create-state-machine-role-arn-length-bounds
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions|CreateStateMachine|4xx->4xx|$.message
Issue-Commit: 43ebd6ce98e7173c42d111a904ff9f0222d298b2

# stepfunctions: CreateStateMachine roleArn ValidationException truncates the echoed value

> _Note: this issue was identified against the code base at commit `43ebd6ce98e7173c42d111a904ff9f0222d298b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStateMachine

## Description
One cause, on service `stepfunctions`.

AWS: CreateStateMachine with a roleArn longer than the member's modelled maximum is refused with 400 ValidationException whose message is `1 validation error detected: Value '<roleArn>' at 'roleArn' failed to satisfy constraint: Member must have length less than or equal to 256`. The fixed text is that whole sentence; the variable parts are the submitted value — reproduced IN FULL, however long it is — the member name `roleArn`, and the member's modelled maximum 256. Smithy (services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json): CreateStateMachine.roleArn is required and targets Arn @length 1..256; CreateStateMachine declares ValidationException (client error, httpError 400, member `message`).

Emulator: the status (400) and the error code are right, but the echoed value is cut off and the emulator appends its own marker `... [<n> bytes]` (here `... [257 bytes]`), where `<n>` is the submitted length. That marker is emulator vocabulary and appears nowhere in AWS's wording.

Why: a client that logs, displays or asserts on the refusal text sees a role ARN it never sent; tooling that extracts the rejected value from the message to point the operator at the offending input recovers a mangled string carrying the emulator's byte count. Because the whole value is omitted, the caller cannot tell from the refusal WHICH role ARN was rejected when several are in play.

Fix: the constraint-validation message renderer behind CreateStateMachine's input validation must interpolate the member value unabridged, with no length cap and no `... [n bytes]` suffix.

Traces: stepfunctions-create-state-machine-role-arn-length-bounds#task#4.

Note: the same renderer fault is reported for the `states` service token in the companion issue titled "states: ValidationException messages misrender the offending value and enum set"; this one is filed separately because it is recorded under a different service token and takes its own verdict.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 108 fixtures, 1146 steps, 42 mismatched, score 96.3%. Divergence key service=stepfunctions|CreateStateMachine|status=400->400|keys=$.message, expected 400 / actual 400:
  expected "1 validation error detected: Value 'arn:aws:iam::000000000000:role/boundary-path/boundary-path/.../bx/sim2real-probe-sfn-24a5a909' at 'roleArn' failed to satisfy constraint: Member must have length less than or equal to 256" (the full 257-byte role ARN inside the quotes)
  got      "1 validation error detected: Value 'arn:aws:iam::000000000000:role/boundary-path/boundary-path/.../bx/sim2real-probe-sfn-24a5a90... [257 bytes]' at 'roleArn' failed to satisfy constraint: Member must have length less than or equal to 256"

The account number, path padding and generated name in the ARN are per-run values; the divergence is the truncation of the echoed value and the added "... [257 bytes]" marker.

Model evidence: CreateStateMachine.roleArn required, target Arn @length min 1 max 256; CreateStateMachine errors include ValidationException (httpError 400, member `message`).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Fixture stepfunctions-create-state-machine-role-arn-length-bounds, step task#4 — CreateStateMachine with a roleArn of 257 bytes.
3. Compare $.message against the recorded response: the quoted value must be the complete submitted roleArn, with no truncation and no "... [n bytes]" marker, and the remainder of the sentence must be unchanged.
4. Re-replay the fixture to confirm the step passes before closing.
