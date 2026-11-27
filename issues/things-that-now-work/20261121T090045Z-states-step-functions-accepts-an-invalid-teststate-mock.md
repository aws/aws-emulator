Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-test-state-flow-and-callback-mock-requirements
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-test-state-flow-and-callback-mock-requirements sfn-validation-reason-labeled-arn-and-completed-map-run
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|TestState|4xx->2xx|$.__type
Issue-Key: states|TestState|4xx->2xx|$.message
Issue-Key: states|TestState|4xx->2xx|status
Issue-Key: states|DescribeExecution|2xx->2xx|$.redriveStatus
Issue-Key: states|DescribeExecution|2xx->2xx|$.redriveStatusReason
Issue-Key: states|DescribeExecution|2xx->2xx|$.status
Issue-Related: 20261029T171911Z-states-step-functions-response-construction-map-run-arns-carry.md
Issue-Commit: 229c05c4a85b849588e172b17631c1308ea16fa2

# states: Step Functions accepts an invalid TestState mock and reports a succeeded distributed-Map execution as FAILED

> _Note: this issue was identified against the code base at commit `229c05c4a85b849588e172b17631c1308ea16fa2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestState

## Description
Two net-new causes on service `states`: the TestState mock validator never refuses a malformed Parallel mock, and an execution whose Map state ran to completion is described as FAILED with no output and a wrong redrive verdict.

Cause 1 of 2: TestState accepts a Parallel mock result whose item count does not match the number of branches.
AWS: when the tested state is a Parallel state and `mock.result` is a JSON array whose length differs from the state's branch count, TestState refuses the request with HTTP 400, __type "com.amazonaws.swf.service.v2.model#ValidationException" and message "Mocked result must contain the same number of items as number of Parallel branches." — the whole sentence is fixed text with no variable part. Smithy (services/service-com-amazonaws-sfn-awsstepfunctions): TestState errors include ValidationException (client, httpError 400); MockInput.result is "A JSON string containing the mocked result of the state invocation" and MockInput.fieldValidationMode defaults to STRICT, i.e. mocked results are validated against the state's shape unless the caller opts out.
Emulator: HTTP 200 with a normal TestState result — no __type and no message at all, so the mock is silently accepted and the state is reported as having been exercised.
Why: a caller using TestState to validate a mocked Parallel fan-out gets a green result for a mock that AWS rejects, so the test harness certifies a definition/mock pair that cannot run; error-path tests that assert on ValidationException never see one, and the 400-vs-200 difference breaks any client that branches on the HTTP status.
Fix: the TestState handler's mock validation (the Parallel-branch arity check in the mock/field-validation stage), which must raise ValidationException with AWS's sentence before the state is simulated.
Traces: sfn-test-state-flow-and-callback-mock-requirements#task#2 (representative) and #task#5.

Cause 2 of 2: DescribeExecution reports a completed Map execution as FAILED, omits its output and marks it REDRIVABLE.
AWS: once every iteration of a Map state has completed, the parent execution reaches `status` = "SUCCEEDED" and DescribeExecution returns the execution's `output` (the Map's result array — for an empty item list, the JSON text "[]"), `outputDetails` = {__type: "com.amazonaws.swf.base.model#CloudWatchEventsExecutionDataDetails", included: true}, `redriveStatus` = "NOT_REDRIVABLE" and `redriveStatusReason` = "Execution is SUCCEEDED and cannot be redriven" — that reason sentence is fixed text apart from the status word it names. Smithy: DescribeExecution output carries status (ExecutionStatus, required), output, outputDetails (CloudWatchEventsExecutionDataDetails), redriveStatus (ExecutionRedriveStatus = REDRIVABLE|NOT_REDRIVABLE|REDRIVABLE_BY_MAP_RUN) and redriveStatusReason.
Emulator: status "FAILED" (the fixture's poll gives up because FAILED is a terminal fail-on state after 2 attempts), `output` and `outputDetails` absent, `redriveStatus` "REDRIVABLE" and `redriveStatusReason` absent.
Why: a client waiting on the execution sees a failure for a workflow that in fact completed, never receives the Map's output, and is told it may redrive an execution AWS considers non-redrivable — so success paths are untestable and redrive logic is driven in the wrong direction.
Fix: Map/Map-Run completion must settle the parent execution: the execution-state machine's Map-state completion handling (status transition to SUCCEEDED and result propagation into the execution's output) and the DescribeExecution serializer's redriveStatus/redriveStatusReason derivation from the terminal status.
Traces: sfn-validation-reason-labeled-arn-and-completed-map-run#task#5. Related open issues, neither of which owns this replay key: 20261112T174000Z-states-describemaprun-reports-a-completed-map-run-as-failed.md (the DescribeMapRun surface of the same completion gap) and 20261029T171911Z-states-step-functions-response-construction-map-run-arns-carry.md (Map Run ARN/response construction).

## Evidence
Replay of trace set simparity-wire-aws-v1 (80 fixtures, 893 steps, 29 mismatched).

Cause 1 — TestState, expected 400 / actual 200 (fixture sfn-test-state-flow-and-callback-mock-requirements, step task#2; cluster also covers task#5):
  status: expected 400, got 200
  $.__type: expected 'com.amazonaws.swf.service.v2.model#ValidationException', got None
  $.message: expected 'Mocked result must contain the same number of items as number of Parallel branches.', got None

Cause 2 — DescribeExecution, expected 200 / actual 200 (fixture sfn-validation-reason-labeled-arn-and-completed-map-run, step task#5):
  $.status: expected 'SUCCEEDED', got 'FAILED' (poll: 'FAILED' is in fail_on after 2 attempts)
  $.output: expected '[]', got None
  $.outputDetails: expected {'__type': 'com.amazonaws.swf.base.model#CloudWatchEventsExecutionDataDetails', 'included': True}, got None
  $.redriveStatus: expected 'NOT_REDRIVABLE', got 'REDRIVABLE'
  $.redriveStatusReason: expected 'Execution is SUCCEEDED and cannot be redriven', got None

Model evidence: TestState declares ValidationException (httpError 400) and MockInput.fieldValidationMode defaults to STRICT; DescribeExecution output declares output/outputDetails/redriveStatus/redriveStatusReason with ExecutionRedriveStatus enum [REDRIVABLE, NOT_REDRIVABLE, REDRIVABLE_BY_MAP_RUN].

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: fixture sfn-test-state-flow-and-callback-mock-requirements, step task#2 — call TestState on a Parallel state with mock.result holding a different number of items than the state has branches. Expect 400 ValidationException with message 'Mocked result must contain the same number of items as number of Parallel branches.'; observe 200 with a normal result. Repeat for step task#5.
3. Cause 2: fixture sfn-validation-reason-labeled-arn-and-completed-map-run, step task#5 — start the Map state machine, then poll DescribeExecution. Expect the execution to settle SUCCEEDED with output '[]', outputDetails.included=true, redriveStatus NOT_REDRIVABLE and redriveStatusReason 'Execution is SUCCEEDED and cannot be redriven'; observe FAILED, no output/outputDetails, redriveStatus REDRIVABLE and no reason.
4. Both causes must pass before closing: re-replay sfn-test-state-flow-and-callback-mock-requirements#task#2 and #task#5 AND sfn-validation-reason-labeled-arn-and-completed-map-run#task#5.
