Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-execution-trace-header-length-bounds
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-execution-trace-header-length-bounds start-sync-execution-trace-header-bounds test-state-retrier-retry-count-range
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|DescribeExecution|2xx->2xx|$.traceHeader
Issue-Key: states|StartSyncExecution|2xx->2xx|$.traceHeader
Issue-Key: states|TestState|2xx->2xx|$.inspectionData.errorDetails
Issue-Commit: 43ebd6ce98e7173c42d111a904ff9f0222d298b2

# states: execution responses drop traceHeader and TestState retry inspection details

> _Note: this issue was identified against the code base at commit `43ebd6ce98e7173c42d111a904ff9f0222d298b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExecution

## Description
Three causes, all on service `states`: the emulator answers 200 with the right shape but leaves modelled response members empty or absent, so data the caller supplied or the state machine produced is never readable back.

Smithy (services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json): DescribeExecutionOutput.traceHeader -> TraceHeader @length 0..256, pattern ^\\p{ASCII}*$; StartSyncExecutionOutput.traceHeader -> the same shape; StartExecutionInput.traceHeader / StartSyncExecutionInput.traceHeader are the matching inputs. TestStateOutput.inspectionData -> InspectionData, whose member `errorDetails` -> InspectionErrorDetails documents "An object containing data about a handled exception in the tested state" with members catchIndex ("The array index of the Catch which handled the exception"), retryIndex ("The array index of the Retry which handled the exception") and retryBackoffIntervalSeconds.

Cause 1 of 3 — DescribeExecution returns an empty traceHeader (representative).
AWS: an execution started with a traceHeader reports that trace header back on DescribeExecution; the `traceHeader` member of the response is populated (non-empty) and carries the X-Ray trace header associated with the execution. Recorded behavior: the recording asserts the member is present and shaped (non-empty), its per-run value being a trace id that differs each run.
Emulator: 200 with `traceHeader` set to the empty string — the member is serialized but carries nothing.
Why: X-Ray correlation is done by reading the execution's trace header back out of the describe response; with an empty value a client cannot tie an execution to its trace, and code that parses `Root=1-...` out of it fails on an empty string.
Fix: the execution record's state must retain the traceHeader supplied at StartExecution (or the one the emulator mints when tracing is enabled) and the DescribeExecution handler must project it into the response.
Traces: sfn-execution-trace-header-length-bounds#task#9.

Cause 2 of 3 — StartSyncExecution returns an empty traceHeader.
AWS: StartSyncExecution echoes the execution's trace header in its own `traceHeader` output member; the recording asserts a present, non-empty value.
Emulator: 200 with `traceHeader` = empty string.
Why: the synchronous surface returns the whole execution result in one response, so this is the only place a sync caller can obtain the trace header; empty means the correlation is lost with no second call available to recover it.
Fix: the StartSyncExecution handler must carry the request's traceHeader (or the minted one) through to its output member.
Traces: start-sync-execution-trace-header-bounds#task#4.

Cause 3 of 3 — TestState omits inspectionData.errorDetails for a retried state.
AWS: when the tested state's invocation fails and a Retry entry handles the exception, TestState's inspectionData includes `errorDetails` naming which handler handled it — `retryIndex` set to the array index of the Retry that fired (0 for the first Retrier).
Emulator: 200 with the rest of inspectionData present but `errorDetails` absent entirely.
Why: TestState exists so an author can see HOW a state behaved; without errorDetails the caller cannot tell that a Retrier fired at all, nor which one, so retry configuration cannot be validated from the test surface and any tooling reading inspectionData.errorDetails.retryIndex sees nothing.
Fix: the TestState inspection path must populate InspectionData.errorDetails (retryIndex, and catchIndex/retryBackoffIntervalSeconds where applicable) whenever a Retry or Catch handled the exception.
Traces: test-state-retrier-retry-count-range#task#5.

All three causes must be closed before submitting: re-replay every fixture#step listed above, not only the representative. The trace-header VALUE is a per-run value and is not asserted; its presence and non-emptiness are the requirement.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 108 fixtures, 1146 steps, 42 mismatched, score 96.3%.

Cause 1 — DescribeExecution, expected 200 / actual 200, key $.traceHeader:
  expected '<shaped-not-asserted>' (recorded response carried a present, shaped trace header), got ''

Cause 2 — StartSyncExecution, 200/200, key $.traceHeader:
  expected '<shaped-not-asserted>', got ''

Cause 3 — TestState, 200/200, key $.inspectionData.errorDetails:
  expected {'retryIndex': 0}, got None

Model evidence: DescribeExecutionOutput.traceHeader and StartSyncExecutionOutput.traceHeader both target TraceHeader (@length 0..256, pattern ^\\p{ASCII}*$); InspectionData.errorDetails targets InspectionErrorDetails{catchIndex, retryIndex, retryBackoffIntervalSeconds}, retryIndex documented as "The array index of the Retry which handled the exception".

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: fixture sfn-execution-trace-header-length-bounds, step task#9 — StartExecution with a valid traceHeader, then DescribeExecution on the returned executionArn; assert the response's traceHeader is present and non-empty.
3. Cause 2: start-sync-execution-trace-header-bounds#task#4 — StartSyncExecution with a valid traceHeader; assert the response's traceHeader is present and non-empty.
4. Cause 3: test-state-retrier-retry-count-range#task#5 — TestState on a state whose invocation fails and whose Retry entry handles the failure; assert inspectionData.errorDetails is present with retryIndex naming the Retrier that fired.
5. All three must replay clean before closing.
