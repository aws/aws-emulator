Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-stackset-drift-detection-refusals
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-stackset-drift-detection-refusals
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DetectStackSetDrift|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudformation|DetectStackSetDrift|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DescribeStackSetOperation|2xx->4xx|$
Issue-Key: cloudformation|DescribeStackSetOperation|2xx->4xx|$.DescribeStackSetOperationResponse
Issue-Key: cloudformation|DescribeStackSetOperation|2xx->4xx|$.DescribeStackSetOperationResponse.DescribeStackSetOperationResult.StackSetOperation.Status
Issue-Key: cloudformation|DescribeStackSetOperation|2xx->4xx|status
Issue-Key: cloudformation|ListStackSetOperations|2xx->2xx|$.ListStackSetOperationsResponse.ListStackSetOperationsResult.Summaries
Issue-Commit: 9e2ba0678642d895992d831c5d4d97ae7316945d

# DetectStackSetDrift on a stack set with no instances is refused, so no DETECT_DRIFT operation is recorded or listable

> _Note: this issue was identified against the code base at commit `9e2ba0678642d895992d831c5d4d97ae7316945d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectStackSetDrift

## Description
Three net-new causes around StackSet drift detection on a stack set that has no stack instances. They share one root — the emulator invents a state guard AWS does not have — but each is a separate observable divergence and a fixer must close all three.

Cause 1 of 3 — DetectStackSetDrift refuses with the wrong error class and message, and refuses on state instead of validating the request (fixture cloudformation-stackset-drift-detection-refusals, step task#7).
AWS: Smithy: DetectStackSetDrift input {StackSetName (required), OperationPreferences, OperationId (idempotencyToken), CallAs}; errors InvalidOperationException(400), OperationInProgressException(409), StackSetNotFoundException(404), plus the service-common query ValidationError. StackSetOperationPreferences documents the mutual exclusion explicitly: "You can specify either FailureToleranceCount or FailureTolerancePercentage, but not both" (and likewise MaxConcurrentCount/MaxConcurrentPercentage). AWS validates the request's OperationPreferences BEFORE it looks at stack-set state, and reports the violation as the query-protocol ValidationError: Error.Type 'Sender', Error.Code 'ValidationError', Error.Message "Exactly one of FailureToleranceCount or FailureTolerancePercentage must be specified" — that message is entirely fixed text, it names no request or state value. A stack set with zero instances is NOT itself a refusal condition (see Cause 2: AWS runs the operation to SUCCEEDED over 0 accounts).
Emulator: the same request is refused 400 with Error.Code 'InvalidOperationException' and Error.Message "StackSet <stack-set-name> has no stack instances to detect drift on" — an emulator-authored message whose only variable part is the stack-set name from the request. The modeled preference validation is never reached, and the invented no-instances guard fires in its place.
Why: a client that submits an invalid OperationPreferences pair cannot tell request-shape errors from state errors — it sees InvalidOperationException (a retry/state class) instead of a Sender ValidationError, and the message points at the stack set rather than at the two conflicting members, so the caller never learns which field to fix.
Fix: the DetectStackSetDrift handler's validation ordering — validate OperationPreferences (FailureToleranceCount vs FailureTolerancePercentage, MaxConcurrentCount vs MaxConcurrentPercentage) and emit ValidationError before any state inspection, and drop the no-stack-instances refusal entirely.
Traces: cloudformation-stackset-drift-detection-refusals#task#7.

Cause 2 of 3 — the OperationId returned/accepted by DetectStackSetDrift is not resolvable: DescribeStackSetOperation answers 404 OperationNotFoundException (fixture cloudformation-stackset-drift-detection-refusals, step task#4).
AWS: Smithy: DetectStackSetDrift returns OperationId (client-suppliable, idempotencyToken, pattern ^[a-zA-Z0-9][-a-zA-Z0-9]*$); DescribeStackSetOperation{StackSetName, OperationId} returns StackSetOperation and raises OperationNotFoundException(404) only when no such operation exists. AWS accepts drift detection on a stack set with zero instances and records a real operation that settles: StackSetOperation with Action 'DETECT_DRIFT', Status 'SUCCEEDED', the submitted OperationId, the stack set's AdministrationRoleARN and ExecutionRoleName, OperationPreferences, CreationTimestamp/EndTimestamp, and StatusDetails {TotalAccountsCount '0', FailedStackInstancesCount '0', Stages.RoleProvisioning.Status 'NOT_APPLICABLE', Stages.StackInstancesDeployment.Status 'NOT_APPLICABLE'} — i.e. a successful no-op over zero accounts.
Emulator: DescribeStackSetOperation for that OperationId answers 404 with Error.Code 'OperationNotFoundException' and Error.Message 'Operation not found', and the harness polls it to the 180 s cap without the operation ever appearing — because the DetectStackSetDrift that should have created it was refused by the guard in Cause 1, so no operation record was ever written.
Why: the documented way to wait for stack-set drift detection is to poll DescribeStackSetOperation on the returned OperationId. A caller gets a permanent 404 instead of a terminal SUCCEEDED, so it either loops until its own timeout or concludes the operation was lost.
Fix: the DetectStackSetDrift handler plus the stack-set operation store — record a DETECT_DRIFT StackSetOperation (populating AdministrationRoleARN, ExecutionRoleName, OperationPreferences, StatusDetails counts and the NOT_APPLICABLE stage statuses) and settle it SUCCEEDED when there are no instances to inspect.
Traces: cloudformation-stackset-drift-detection-refusals#task#4.

Cause 3 of 3 — ListStackSetOperations reports the stack set as having no operations at all (fixture cloudformation-stackset-drift-detection-refusals, step task#6).
AWS: Smithy: ListStackSetOperations{StackSetName (required)} returns Summaries, a list of StackSetOperationSummary{OperationId, Action, Status, CreationTimestamp, EndTimestamp, StatusReason, StatusDetails, OperationPreferences}; only StackSetNotFoundException(404) is modeled. Every operation performed against the stack set appears there, so the drift-detection operation is listed with Action 'DETECT_DRIFT', Status 'SUCCEEDED' and the same StatusDetails (TotalAccountsCount 0, FailedStackInstancesCount 0, both stages NOT_APPLICABLE).
Emulator: ListStackSetOperationsResult.Summaries is serialized as an empty value (type_mismatch: expected a member list, got ''), so the drift operation is absent from the stack set's operation history.
Why: a client reconciling a stack set's operation history — to find the last DETECT_DRIFT, to check whether an operation is still running before issuing another, or to audit what ran — sees nothing and concludes no operation was ever performed.
Fix: the ListStackSetOperations handler/serializer over the stack-set operation store; it must list DETECT_DRIFT operations alongside CREATE/UPDATE/DELETE ones, and must serialize a populated Summaries list rather than an empty element.
Traces: cloudformation-stackset-drift-detection-refusals#task#6.

## Evidence
Cause 1 (DetectStackSetDrift, cloudformation-stackset-drift-detection-refusals#task#7), both sides HTTP 400:
  expected $.ErrorResponse.Error.Code    = 'ValidationError'          (Error.Type 'Sender')
  actual   $.ErrorResponse.Error.Code    = 'InvalidOperationException'
  expected $.ErrorResponse.Error.Message = 'Exactly one of FailureToleranceCount or FailureTolerancePercentage must be specified'  (fixed text, no variable parts)
  actual   $.ErrorResponse.Error.Message = 'StackSet sim2real-probe-cfn-ssdrift-6a346c8b has no stack instances to detect drift on'  (fixed text 'StackSet ... has no stack instances to detect drift on' + the stack-set name from the request; the name is a per-run value)

Cause 2 (DescribeStackSetOperation, cloudformation-stackset-drift-detection-refusals#task#4):
  expected status 200 with DescribeStackSetOperationResult.StackSetOperation = {Action 'DETECT_DRIFT', Status 'SUCCEEDED', OperationId <submitted id>, StackSetId, AdministrationRoleARN, ExecutionRoleName, OperationPreferences, StatusDetails {TotalAccountsCount '0', FailedStackInstancesCount '0', Stages.RoleProvisioning.Status 'NOT_APPLICABLE', Stages.StackInstancesDeployment.Status 'NOT_APPLICABLE'}, CreationTimestamp, EndTimestamp}
  actual   status 404, $.ErrorResponse.Error.Code 'OperationNotFoundException', Error.Message 'Operation not found'; polled to the 180 s cap without ever resolving

Cause 3 (ListStackSetOperations, cloudformation-stackset-drift-detection-refusals#task#6), status 200 both sides:
  $.ListStackSetOperationsResponse.ListStackSetOperationsResult.Summaries
  expected member list containing the DETECT_DRIFT operation (Action 'DETECT_DRIFT', Status 'SUCCEEDED', StatusDetails as above)
  actual   '' (empty, no members)

## Steps to Reproduce
1. Create a stack set with an administration role and execution role and add NO stack instances.
2. Call DetectStackSetDrift with a client-supplied OperationId -> AWS accepts it (200, OperationId echoed); the emulator refuses 400 InvalidOperationException 'StackSet <name> has no stack instances to detect drift on'.
3. Poll DescribeStackSetOperation{StackSetName, OperationId} for that id -> AWS settles SUCCEEDED with StatusDetails.TotalAccountsCount 0 and both stages NOT_APPLICABLE; the emulator answers 404 OperationNotFoundException 'Operation not found' until the 180 s cap (Cause 2).
4. Call ListStackSetOperations{StackSetName} -> AWS lists the DETECT_DRIFT summary; the emulator returns empty Summaries (Cause 3).
5. Call DetectStackSetDrift again supplying BOTH OperationPreferences.FailureToleranceCount and FailureTolerancePercentage -> AWS answers 400 Sender/ValidationError 'Exactly one of FailureToleranceCount or FailureTolerancePercentage must be specified'; the emulator answers 400 InvalidOperationException with the no-stack-instances message (Cause 1).
Fixture: cloudformation-stackset-drift-detection-refusals, steps task#4, task#6, task#7.
