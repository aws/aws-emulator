Issue-Source: trace-sweep-divergence
Issue-Trace: describe-state-machine-for-execution-arn-length-bound
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: describe-state-machine-for-execution-arn-length-bound sfn-create-state-machine-rejects-unknown-aws-sdk-operation sfn-create-state-machine-rejects-unknown-aws-sdk-service sfn-list-executions-unknown-parent-not-found states-list-executions-next-token-length-bounds states-map-run-listings-max-results-bounds
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|DescribeStateMachineForExecution|4xx->4xx|$.__type
Issue-Key: states|CreateStateMachine|4xx->2xx|$.__type
Issue-Key: states|CreateStateMachine|4xx->2xx|$.message
Issue-Key: states|CreateStateMachine|4xx->2xx|status
Issue-Key: states|ListExecutions|4xx->4xx|$.message
Issue-Key: states|ListExecutions|2xx->2xx|$.executions[*].executionArn
Issue-Related: 20261127T165543Z-states-validationexception-messages-misrender-the-offending-value-and.md
Issue-Commit: 43ebd6ce98e7173c42d111a904ff9f0222d298b2

# states: ARN validation and response ARN construction diverge across operations

> _Note: this issue was identified against the code base at commit `43ebd6ce98e7173c42d111a904ff9f0222d298b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStateMachineForExecution

## Description
Four causes, all on service `states`, all rooted in how the emulator parses, refuses and builds Step Functions ARNs: a malformed ARN is screened in the wrong order, an unrecognized service-integration resource ARN is not screened at all, a refusal echoes a qualified ARN where AWS names the unqualified one, and a Map Run child execution's ARN is built with the wrong ARN type and shape.

Smithy (services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json): DescribeStateMachineForExecution declares errors InvalidArn (client, 400) and ExecutionDoesNotExist (400), input executionArn -> Arn @length 1..256. CreateStateMachine declares InvalidDefinition (client, 400, doc "The provided Amazon States Language definition is not valid."). ListExecutions declares StateMachineDoesNotExist (400, doc "The specified state machine does not exist.") and input stateMachineArn -> Arn. ExecutionListItem.executionArn -> Arn (required) and ExecutionListItem.mapRunArn -> LongArn, documented as returned only when mapRunArn was specified in the ListExecutions call.

Cause 1 of 4 — a structurally invalid ARN is refused as a length violation instead of InvalidArn (representative).
AWS: ARN syntax is screened BEFORE the length constraint. For an executionArn whose resource path is not a valid execution path (here an ARN missing its region segment, `arn:aws:states::<account>:execution:<...>`), AWS answers 400 with __type `com.amazonaws.swf.service.v2.model#InvalidArn` and message `Invalid Arn: 'Resource path not valid: <the submitted ARN>'`. The fixed text is `Invalid Arn: 'Resource path not valid: ` + the ARN + `'`; the only variable part is the submitted ARN, reproduced in full.
Emulator: 400 with __type `ValidationException` and the generic length message `1 validation error detected: Value '<truncated ARN>' at 'executionArn' failed to satisfy constraint: Member must have length less than or equal to 256`.
Why: the client receives a different modelled error shape than AWS — code `ValidationException` instead of `InvalidArn` — so SDK error-type matching, retry classification and catch blocks keyed on InvalidArn never fire, and the diagnosis points at length rather than at the malformed resource path.
Fix: the ARN-screening stage of the DescribeStateMachineForExecution handler's input validation must run the ARN-shape check ahead of the generic @length constraint check and raise InvalidArn with AWS's "Resource path not valid" sentence.
Traces: describe-state-machine-for-execution-arn-length-bound#task#9.

Cause 2 of 4 — CreateStateMachine accepts a definition whose Task Resource names an unrecognized aws-sdk service or operation.
AWS: a Task state whose Resource is `arn:aws:states:::aws-sdk:<service>:<operation>` where that service or operation does not exist is refused at create time: 400, __type `com.amazonaws.swf.service.v2.model#InvalidDefinition`, message `Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: The resource provided <resource-arn> is not recognized. The value is not a valid resource ARN, or the resource is not available in this region. at <location>'`. Fixed text is the `Invalid State Machine Definition: '<CODE>: ...'` envelope and the sentence `The resource provided <arn> is not recognized. The value is not a valid resource ARN, or the resource is not available in this region.`; the variable parts are the offending Resource ARN and the `/States/<StateName>/Resource` location.
Emulator: 200 — the state machine is created, with no __type and no message.
Why: a typo'd integration service or operation name is accepted, so the failure moves from a deterministic create-time 400 to a runtime failure (or silent success) later; a client's deploy-time validation gate passes a workflow AWS refuses.
Fix: the service-integration Resource resolver consulted by CreateStateMachine's definition validation must reject unknown `aws-sdk:<service>:<operation>` pairs and surface them as InvalidDefinition/SCHEMA_VALIDATION_FAILED.
Traces: sfn-create-state-machine-rejects-unknown-aws-sdk-operation#task#2 (cluster also covers sfn-create-state-machine-rejects-unknown-aws-sdk-service#task#2, the unknown-service variant).

Cause 3 of 4 — StateMachineDoesNotExist echoes the qualified (alias/version) ARN instead of the state machine ARN.
AWS: when ListExecutions is called for a state machine that does not exist, the refusal is 400 StateMachineDoesNotExist with message `State Machine Does Not Exist: '<state-machine-arn>'`. The fixed text is `State Machine Does Not Exist: '` + ARN + `'`, and the ARN named is the UNQUALIFIED state machine ARN — the alias or version qualifier the caller appended is stripped before the ARN is quoted.
Emulator: same status and code, but quotes the caller's qualified ARN with the `:<alias>` suffix still attached.
Why: the identifier in the error does not match the resource AWS reports as missing, so a client that parses the ARN out of the message to retry or to report the resource gets a qualified ARN that AWS would never return there.
Fix: the not-found path of the ListExecutions handler must strip the version/alias qualifier from the resolved state machine ARN before rendering the message.
Traces: sfn-list-executions-unknown-parent-not-found#task#2 (cluster also covers sfn-list-executions-unknown-parent-not-found#task#3 and states-list-executions-next-token-length-bounds#task#10).

Cause 4 of 4 — Map Run child executions are listed with the wrong executionArn type and shape.
AWS: ListExecutions called with a `mapRunArn` returns the Map Run's CHILD workflow executions, and for an EXPRESS Map Run each executionArn uses the express resource type and carries the map-run and child-execution segments: `arn:aws:states:<region>:<account>:express:<state-machine-name>/<map-run-id>:<child-execution-name>:<child-execution-id>`.
Emulator: builds `arn:aws:states:<region>:<account>:execution:<state-machine-name>/<id>:<id>` — the `execution` resource type instead of `express`, and one fewer identifier segment.
Why: the ARN is the handle a client uses for follow-up calls (DescribeExecution, GetExecutionHistory, redrive) and for correlating child executions back to their Map Run; an ARN with the wrong resource type and arity is unusable and cannot be matched against the ARN form CloudWatch/EventBridge records carry.
Fix: the ARN construction used when the ListExecutions handler projects Map Run child executions into ExecutionListItem — it must select the resource type from the state machine/Map Run type (express for EXPRESS) and emit the full map-run/child-execution segment structure.
Traces: states-map-run-listings-max-results-bounds#task#9.

All four causes must be closed before submitting: re-replay every fixture#step listed above, not only the representative. Generated ids, account numbers, regions and names in the quoted ARNs above are per-run values, not requirements — the resource type, segment structure and the fixed sentence text are.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 108 fixtures, 1146 steps, 42 mismatched, score 96.3%. Expected = recorded AWS response, actual = emulator.

Cause 1 — DescribeStateMachineForExecution, expected 400 / actual 400, keys $.__type,$.message:
  $.__type: expected 'com.amazonaws.swf.service.v2.model#InvalidArn', got 'ValidationException'
  $.message: expected "Invalid Arn: 'Resource path not valid: arn:aws:states::000000000000:execution:execution-arn-length-probe-...-ez'", got "1 validation error detected: Value 'arn:aws:states::000000000000:execution:execution-arn-length-probe-...e... [257 bytes]' at 'executionArn' failed to satisfy constraint: Member must have length less than or equal to 256"

Cause 2 — CreateStateMachine, expected 400 / actual 200, keys $.__type,$.message,status:
  status: expected 400, got 200
  $.__type: expected 'com.amazonaws.swf.service.v2.model#InvalidDefinition', got None
  $.message: expected "Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: The resource provided arn:aws:states:::aws-sdk:sqs:noSuchOperation is not recognized. The value is not a valid resource ARN, or the resource is not available in this region. at /States/StateName/Resource'", got None
  cluster affected_count = 2 (sfn-create-state-machine-rejects-unknown-aws-sdk-operation#task#2, sfn-create-state-machine-rejects-unknown-aws-sdk-service#task#2)

Cause 3 — ListExecutions, 400/400, key $.message:
  expected "State Machine Does Not Exist: 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-probe-sfnlx-b17d0980'"
  got      "State Machine Does Not Exist: 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-probe-sfnlx-b17d0980:live'"
  cluster affected_count = 3 (sfn-list-executions-unknown-parent-not-found#task#2, #task#3, states-list-executions-next-token-length-bounds#task#10)

Cause 4 — ListExecutions, 200/200, key $.executions[0].executionArn:
  expected 'arn:aws:states:us-west-2:000000000000:express:sim2real-probe-sfn-mrpage-ef31d96d/add01803-87a0-385c-b247-1180b375b8d9:b7a1d1b2-dc1e-3b91-beb4-2d81f947d4a5:cbf600c3-5d96-410d-8183-0ebe6f260f1b'
  got      'arn:aws:states:us-west-2:000000000000:execution:sim2real-probe-sfn-mrpage-ef31d96d/a0edc67a-c3af-3459-aea0-c0abc02fc424:ea52a19a-816d-41a6-896c-d8d6fdb8f473'
  (the uuids differ per run; the divergence is the `express` vs `execution` resource type and the missing identifier segment)

Model evidence: DescribeStateMachineForExecution errors include InvalidArn (400) and executionArn -> Arn @length 1..256; CreateStateMachine errors include InvalidDefinition (400); ListExecutions errors include StateMachineDoesNotExist (400); ExecutionListItem.executionArn -> Arn required, ExecutionListItem.mapRunArn -> LongArn returned only when mapRunArn was specified.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: fixture describe-state-machine-for-execution-arn-length-bound, step task#9 — DescribeStateMachineForExecution with an executionArn that is both over 256 bytes and structurally invalid (region segment empty). Expect 400 InvalidArn with "Invalid Arn: 'Resource path not valid: <arn>'"; observe 400 ValidationException with the length message.
3. Cause 2: sfn-create-state-machine-rejects-unknown-aws-sdk-operation#task#2 — CreateStateMachine with a Task whose Resource is arn:aws:states:::aws-sdk:sqs:noSuchOperation. Expect 400 InvalidDefinition; observe 200. Repeat for sfn-create-state-machine-rejects-unknown-aws-sdk-service#task#2 (unknown service).
4. Cause 3: sfn-list-executions-unknown-parent-not-found#task#2 — ListExecutions for a non-existent state machine addressed by a qualified (alias) ARN; assert the message quotes the unqualified state machine ARN. Repeat for #task#3 and states-list-executions-next-token-length-bounds#task#10.
5. Cause 4: states-map-run-listings-max-results-bounds#task#9 — ListExecutions with mapRunArn for an EXPRESS Map Run; assert each executions[].executionArn uses the `express` resource type and carries the state-machine/map-run/child-execution segment structure.
6. All four must replay clean before closing.
