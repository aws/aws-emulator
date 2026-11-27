Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-execution-trace-header-length-bounds
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-execution-trace-header-length-bounds start-execution-trace-header-length-and-charset stepfunctions-start-execution-trace-header-length-limit start-sync-execution-trace-header-bounds describe-state-machine-for-execution-arn-length-bound sfn-activity-list-next-token-length-bounds states-delete-state-machine-arn-length-bounds states-delete-state-machine-version-arn-length-bounds test-state-role-arn-length-bound describe-map-run-arn-length-bounds create-state-machine-log-level-outside-enum
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|StartExecution|4xx->4xx|$.message
Issue-Key: states|StartSyncExecution|4xx->4xx|$.message
Issue-Key: states|DescribeStateMachineForExecution|4xx->4xx|$.message
Issue-Key: states|ListActivities|4xx->4xx|$.message
Issue-Key: states|DeleteStateMachine|4xx->4xx|$.message
Issue-Key: states|DeleteStateMachineVersion|4xx->4xx|$.message
Issue-Key: states|TestState|4xx->4xx|$.message
Issue-Key: states|DescribeMapRun|4xx->4xx|$.message
Issue-Key: states|UpdateStateMachine|4xx->4xx|$.message
Issue-Commit: 43ebd6ce98e7173c42d111a904ff9f0222d298b2

# states: ValidationException messages misrender the offending value and enum set

> _Note: this issue was identified against the code base at commit `43ebd6ce98e7173c42d111a904ff9f0222d298b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
The shared renderer that builds Step Functions' `ValidationException` message diverges from AWS in two ways, across every operation that has a length-constrained or enum-constrained input member. The HTTP status (400) and the error code are correct in all nine causes below; only the message text is wrong, and the message text is part of the API contract.

Fault A (causes 1-8): AWS's constraint message echoes the offending value IN FULL, however long it is. The emulator instead cuts the echoed value off and appends its own marker `... [<n> bytes]`, where `<n>` is the submitted length. The fixed text of the message is `1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must have length less than or equal to <max>`; the variable parts are the submitted value (verbatim, untruncated), the member name and the member's modelled maximum. The marker `... [<n> bytes]` is emulator vocabulary that appears nowhere in AWS's wording. Because this is one renderer, every length-bounded member of every operation is affected; the eight causes below are the recorded instances.

Fault B (cause 9): the enum-constraint message lists the accepted values in the wrong ORDER.

Smithy (services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json): ValidationException is a client error, httpError 400, member `message`. The bounded members involved are TraceHeader @length 0..256 (StartExecution.traceHeader, StartSyncExecution.traceHeader), Arn @length 1..256 (DescribeStateMachineForExecution.executionArn, DeleteStateMachine.stateMachineArn, TestState.roleArn), PageToken @length 1..1024 (ListActivities.nextToken), LongArn @length 1..2000 (DescribeMapRun.mapRunArn, DeleteStateMachineVersion.stateMachineVersionArn), and LogLevel (enum ALL|ERROR|FATAL|OFF).

Cause 1 of 9 — StartExecution.traceHeader (representative).
AWS: 400 ValidationException, message `1 validation error detected: Value '<traceHeader>' at 'traceHeader' failed to satisfy constraint: Member must have length less than or equal to 256`, with the whole submitted 257-byte header reproduced inside the quotes.
Emulator: same status and code, but the quoted value stops early and is followed by `... [257 bytes]`.
Why: a client that logs or asserts on the refusal text sees a value it never sent, and tooling that extracts the rejected value from the message to show the user recovers a mangled string carrying the emulator's byte-count annotation.
Fix: the shared constraint-validation message renderer used by the sfn crate's input validation — it must interpolate the member value unabridged, with no length cap and no `... [n bytes]` suffix.
Traces: sfn-execution-trace-header-length-bounds#task#3 (cluster also covers start-execution-trace-header-length-and-charset#task#6 and stepfunctions-start-execution-trace-header-length-limit#task#6).

Cause 2 of 9 — StartSyncExecution.traceHeader. Same fault on the sync surface: AWS reproduces the full 257-byte traceHeader at 'traceHeader' with the `less than or equal to 256` clause; the emulator truncates it and appends `... [257 bytes]`. Fix as cause 1. Traces: start-sync-execution-trace-header-bounds#task#5.

Cause 3 of 9 — DescribeStateMachineForExecution.executionArn. AWS reproduces the full over-long execution ARN at 'executionArn' with `Member must have length less than or equal to 256`; the emulator truncates it and appends `... [257 bytes]`. Fix as cause 1. Traces: describe-state-machine-for-execution-arn-length-bound#task#8.

Cause 4 of 9 — ListActivities.nextToken. AWS reproduces the full 1025-byte token at 'nextToken' with `Member must have length less than or equal to 1024`; the emulator truncates it and appends `... [1025 bytes]`. Fix as cause 1. Traces: sfn-activity-list-next-token-length-bounds#task#6.

Cause 5 of 9 — DeleteStateMachine.stateMachineArn. AWS reproduces the full over-long value at 'stateMachineArn' with `Member must have length less than or equal to 256`; the emulator truncates it and appends `... [257 bytes]`. Fix as cause 1. Traces: states-delete-state-machine-arn-length-bounds#task#6.

Cause 6 of 9 — DeleteStateMachineVersion.stateMachineVersionArn (LongArn, max 2000). AWS reproduces the full submitted value inside the quotes; the emulator's message diverges on that echoed value ($.message value_mismatch against the recorded response), which is the same renderer as causes 1-5. Fix as cause 1. Traces: states-delete-state-machine-version-arn-length-bounds#task#5.

Cause 7 of 9 — DescribeMapRun.mapRunArn (LongArn, max 2000). AWS reproduces the full submitted map-run reference inside the quotes; the emulator's message diverges on that echoed value ($.message value_mismatch against the recorded response) from the same renderer. Fix as cause 1. Traces: describe-map-run-arn-length-bounds#task#2.

Cause 8 of 9 — TestState.roleArn. AWS reproduces the full over-long role ARN at 'roleArn' with `Member must have length less than or equal to 256`; the emulator truncates it and appends `... [257 bytes]`. Fix as cause 1. Traces: test-state-role-arn-length-bound#task#4.

Cause 9 of 9 — UpdateStateMachine.loggingConfiguration.level, enum set order.
AWS: 400 ValidationException, message `1 validation error detected: Value '<submitted>' at 'loggingConfiguration.level' failed to satisfy constraint: Member must satisfy enum value set: [ALL, FATAL, ERROR, OFF]`. The bracketed list is FIXED text in AWS's declaration order ALL, FATAL, ERROR, OFF; only the quoted submitted value varies.
Emulator: `... Member must satisfy enum value set: [ALL, ERROR, FATAL, OFF]` — the set sorted alphabetically. Hypothesis: the renderer emits the enum members in sorted/parsed order rather than the service model's declaration order.
Why: a client matching the refusal text (or a test asserting it) does not match, and the list ordering misrepresents the model's member order.
Fix: the enum-constraint branch of the same validation message renderer must emit the accepted values in the model's declaration order.
Traces: create-state-machine-log-level-outside-enum#task#6 (UpdateStateMachine). The sibling CreateStateMachine step of the same fixture (task#2) is already owned by open issue 20261121T090045Z-states-asl-definition-validation-misses-and-miswords-schema.md; this UpdateStateMachine cluster is NOT covered by it.

All nine causes must be closed before submitting: re-replay every fixture#step listed above, not only the representative.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 108 fixtures, 1146 steps, 42 mismatched, score 96.3%. Expected values are the recorded AWS responses; actual values are the emulator's.

Cause 1 — StartExecution, expected 400 / actual 400, key $.message:
  expected "1 validation error detected: Value 'Root=1-00000000-000000000000000000000000;Sampled=0;Note=tttt...<full 257-byte header>' at 'traceHeader' failed to satisfy constraint: Member must have length less than or equal to 256"
  got      "1 validation error detected: Value 'Root=1-00000000-000000000000000000000000;Sampled=0;Note=tttt...tttt... [257 bytes]' at 'traceHeader' failed to satisfy constraint: Member must have length less than or equal to 256"
  cluster affected_count = 3 (sfn-execution-trace-header-length-bounds#task#3, start-execution-trace-header-length-and-charset#task#6, stepfunctions-start-execution-trace-header-length-limit#task#6).

Cause 2 — StartSyncExecution, 400/400, $.message: expected the full 'Root=1-6a1f0c00-...;Pad=zzz...' value; got the same value cut off with "... [257 bytes]".

Cause 3 — DescribeStateMachineForExecution, 400/400, $.message: expected the full 'arn:aws:states:us-east-1:000000000000:execution:...missinz' value at 'executionArn'; got it cut off with "... [257 bytes]".

Cause 4 — ListActivities, 400/400, $.message: expected the full 1025-'Q' token at 'nextToken' with the 1024 clause; got it cut off with "... [1025 bytes]".

Cause 5 — DeleteStateMachine, 400/400, $.message: expected the full 257-'q' value at 'stateMachineArn'; got it cut off with "... [257 bytes]".

Cause 6 — DeleteStateMachineVersion, 400/400, divergence_key service=states|DeleteStateMachineVersion|status=400->400|keys=$.message: the emulator's message does not reproduce the recorded message's echoed 'vvvv...' value.

Cause 7 — DescribeMapRun, 400/400, divergence_key service=states|DescribeMapRun|status=400->400|keys=$.message: the emulator's message does not reproduce the recorded message's echoed 'map-run-reference-qqq...' value.

Cause 8 — TestState, 400/400, $.message: expected the full 'arn:aws:iam::000000000000:role/role-arn-length-padding...' value at 'roleArn'; got it cut off with "... [257 bytes]".

Cause 9 — UpdateStateMachine, 400/400, $.message:
  expected "1 validation error detected: Value 'VERBOSE' at 'loggingConfiguration.level' failed to satisfy constraint: Member must satisfy enum value set: [ALL, FATAL, ERROR, OFF]"
  got      "1 validation error detected: Value 'VERBOSE' at 'loggingConfiguration.level' failed to satisfy constraint: Member must satisfy enum value set: [ALL, ERROR, FATAL, OFF]"

Model evidence: TraceHeader @length 0..256 pattern ^\p{ASCII}*$; Arn @length 1..256; PageToken @length 1..1024; LongArn @length 1..2000; LogLevel enum ALL|ERROR|FATAL|OFF; ValidationException httpError 400 with member `message`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: fixture sfn-execution-trace-header-length-bounds, step task#3 — StartExecution with a 257-byte traceHeader. Compare $.message: the echoed value must be the full header, with no "... [n bytes]" marker. Same for start-execution-trace-header-length-and-charset#task#6 and stepfunctions-start-execution-trace-header-length-limit#task#6.
3. Cause 2: start-sync-execution-trace-header-bounds#task#5 — StartSyncExecution with a 257-byte traceHeader.
4. Cause 3: describe-state-machine-for-execution-arn-length-bound#task#8 — DescribeStateMachineForExecution with a 257-byte executionArn.
5. Cause 4: sfn-activity-list-next-token-length-bounds#task#6 — ListActivities with a 1025-byte nextToken.
6. Cause 5: states-delete-state-machine-arn-length-bounds#task#6 — DeleteStateMachine with a 257-byte stateMachineArn.
7. Cause 6: states-delete-state-machine-version-arn-length-bounds#task#5 — DeleteStateMachineVersion with an over-long stateMachineVersionArn.
8. Cause 7: describe-map-run-arn-length-bounds#task#2 — DescribeMapRun with an over-long mapRunArn.
9. Cause 8: test-state-role-arn-length-bound#task#4 — TestState with a 257-byte roleArn.
10. Cause 9: create-state-machine-log-level-outside-enum#task#6 — UpdateStateMachine with loggingConfiguration.level = 'VERBOSE'; assert the enum set is listed as [ALL, FATAL, ERROR, OFF].
11. All nine steps must replay clean before closing.
