Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: arc-region-switch.approval-step-timeout-pauses-execution-by-failed-step arc-region-switch.skip-timed-out-approval-step-completes-with-exceptions arc-region-switch.skip-not-started-step-rejected arc-region-switch.get-plan-execution-unknown-execution-id-not-found arc-region-switch.get-plan-execution-steps-follow-approvals-unissued-execution-rejected
Issue-Shape-Id: com.amazonaws.arcregionswitch#ArcRegionSwitch
Issue-Key: arc-region-switch.approval-step-timeout-pauses-execution-by-failed-step|run#13|AssertionError|assert paused["executionState"] == "pausedByFailedStep", (
Issue-Key: arc-region-switch.skip-timed-out-approval-step-completes-with-exceptions|run#13|AssertionError|assert paused["executionState"] == "pausedByFailedStep", (
Issue-Key: arc-region-switch.skip-not-started-step-rejected|run#14|AssertionError|assert waiting == ("pausedByFailedStep", {first: "failed", second: "notStarted"}), (
Issue-Key: arc-region-switch.get-plan-execution-unknown-execution-id-not-found|run#15|AssertionError|assert got == ("ResourceNotFoundException", f"Execution not found for execution: {never_ran}"), (
Issue-Key: arc-region-switch.get-plan-execution-steps-follow-approvals-unissued-execution-rejected|run#29|AssertionError|assert got == want, f"an execution id the plan never issued gave {got!r}, expected {want!r}"
Issue-Commit: 85a8ef40f44f446d2f7f13d4131def4b1abce9f7

# arc-region-switch: a timed-out manual-approval step terminates the execution (planExecutionTimedOut/canceled) instead of pausing it (pausedByFailedStep/failed), and GetPlanExecution's unknown-execution refusal message is wrong

> _Note: this issue was identified against the code base at commit `85a8ef40f44f446d2f7f13d4131def4b1abce9f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPlanExecution

## Description
Two net-new root causes in the arc-region-switch emulator, both grounded in failing behavioral checks. Both belong to the same service and are bundled here; a fixer must close BOTH before submitting.

Cause 1 of 2: a manual-approval step whose approval window closes ends the whole plan execution instead of pausing it at the failed step.
Failing checks: arc-region-switch.approval-step-timeout-pauses-execution-by-failed-step, arc-region-switch.skip-timed-out-approval-step-completes-with-exceptions, arc-region-switch.skip-not-started-step-rejected.
AWS: recorded behavior (the Smithy model supplies the vocabulary, not the transition). Smithy: ExecutionApprovalConfiguration.timeoutMinutes @range(min:1) @default(60) is the window the approval decision has; ExecutionState enum contains both pausedByFailedStep and planExecutionTimedOut; StepStatus enum contains both failed and canceled; ExecutionEventType enum contains stepFailed and stepPausedByError; GetPlanExecution.endTime is optional and documented as "The time (UTC) when the plan execution ended". Per the recordings, when an approval step is neither approved nor declined inside its own timeoutMinutes window, AWS fails THAT STEP and PAUSES the execution: executionState becomes pausedByFailedStep, the step's status becomes failed (later steps stay notStarted), no endTime is reported (the execution is not terminal), the event stream gains stepFailed and stepPausedByError and no executionSucceeded, ListPlanExecutions(state=pausedByFailedStep) lists the execution while ListPlanExecutions(state=pendingManualApproval) no longer does, and the execution remains open to the operator: CancelPlanExecution ends it as canceled, and UpdatePlanExecutionStep(actionToTake=skip) on the failed step marks that step skipped and lets the workflow continue, ending an execution whose only step was skipped as completedWithExceptions with a stepSkipped event and listing it under ListPlanExecutions(state=completedWithExceptions). While the execution stands paused, UpdatePlanExecutionStep(actionToTake=skip) on a LATER step that has not started is refused with ValidationException and the message "Wait for the step to start before skipping" (fixed text, no interpolation), and that refusal changes nothing in the execution.
Emulator: the closed approval window is treated as a terminal failure of the execution. GetPlanExecution reports executionState 'planExecutionTimedOut' with the approval step's status 'canceled' and an endTime set; the replay read ('planExecutionTimedOut', {'FirstGate': 'canceled', 'SecondGate': 'notStarted'}) where AWS gives ('pausedByFailedStep', {'FirstGate': 'failed', 'SecondGate': 'notStarted'}). Because planExecutionTimedOut is in the emulator's terminal set, every follow-on step of the recorded behavior is unreachable: the execution cannot be resumed or skipped forward, so it never reaches completedWithExceptions and the stepFailed / stepPausedByError / stepSkipped events are never emitted (the emulator records executionFailed instead).
Why: a client driving a Region switch loses the documented operator recovery path. An approval nobody signed off is a pause an operator is meant to resolve (approve, skip or cancel); the emulator turns it into a dead, terminal execution, so runbooks and automation that poll for pausedByFailedStep and then skip the gate hang forever or report a failed switch, and ListPlanExecutions filters by state return the execution under the wrong bucket.
Fix: the execution clock's approval-window transition and the paused-state handling in the service state machine (the Advance::ApprovalWindowClosed arm of advance_execution and the terminal/paused state sets it feeds), plus UpdatePlanExecutionStep's skip handling and its not-started refusal; the GetPlanExecution handler only reports what the state machine stored.
Traces: arc-region-switch.approval-step-timeout-pauses-execution-by-failed-step ('planExecutionTimedOut' where 'pausedByFailedStep' is expected); arc-region-switch.skip-timed-out-approval-step-completes-with-exceptions (same, before the skip); arc-region-switch.skip-not-started-step-rejected (('planExecutionTimedOut', {FirstGate: 'canceled'}) where ('pausedByFailedStep', {FirstGate: 'failed'}) is expected). Code citations: services/service-com-amazonaws-arcregionswitch-arcregionswitch/src/state.rs lines 1544-1559 (ApprovalWindowClosed sets PLAN_EXECUTION_TIMED_OUT, STEP_CANCELED, end_time and calls finish_execution), lines 85-91 (PAUSED_BY_FAILED_STEP documented as "Nothing in this emulator produces it"), lines 347-354 (PLAN_EXECUTION_TIMED_OUT in TERMINAL_EXECUTION_STATES).

Cause 2 of 2: GetPlanExecution's ResourceNotFoundException message for an execution id the plan never issued does not match AWS's wording.
Failing checks: arc-region-switch.get-plan-execution-unknown-execution-id-not-found, arc-region-switch.get-plan-execution-steps-follow-approvals-unissued-execution-rejected.
AWS: Smithy: GetPlanExecution errors ResourceNotFoundException (httpError 404, required member message), "The specified resource was not found." Recorded behavior for the message: code ResourceNotFoundException, HTTP 404, message "Execution not found for execution: <executionId-without-its-region-prefix>" — the fixed text is "Execution not found for execution: " and the only variable part is the hex component of the requested executionId (an execution id is "<region>/<16 hex digits>"; the refusal names only the hex part). The plan ARN does not appear in the message.
Emulator: answers ResourceNotFoundException (correct code and status) with the message "No execution <full executionId including the region prefix> found for plan <planArn>" — different fixed text, the region-prefixed id rather than its hex part, and an extra interpolation of the plan ARN.
Why: a client that matches or logs the error text (or parses the id out of it) for an execution id that was never issued sees a message AWS never produces, so error-text assertions, operator runbooks and log scrapers diverge between the emulator and AWS.
Fix: the GetPlanExecution handler's not-found branch (the message it formats when the execution key is absent from the store).
Traces: arc-region-switch.get-plan-execution-unknown-execution-id-not-found (got 'No execution us-west-2/<hex> found for plan arn:aws:arc-region-switch::<account>:plan/<name>:<suffix>', expected 'Execution not found for execution: <hex>'); arc-region-switch.get-plan-execution-steps-follow-approvals-unissued-execution-rejected (same divergence). Code citation: services/service-com-amazonaws-arcregionswitch-arcregionswitch/src/ops/get_plan_execution.rs lines 213-223.

## Evidence
Cause 1 of 2 — approval-window timeout:
  check arc-region-switch.approval-step-timeout-pauses-execution-by-failed-step
    expected: executionState 'pausedByFailedStep', stepStates {'AwaitSignOff': 'failed'}, no endTime, events including stepFailed and stepPausedByError, listed under ListPlanExecutions(state=pausedByFailedStep), then canceled by CancelPlanExecution
    actual:   AssertionError: an approval step past its step timeout left the execution in 'planExecutionTimedOut', expected 'pausedByFailedStep'
  check arc-region-switch.skip-timed-out-approval-step-completes-with-exceptions
    expected: 'pausedByFailedStep' before the skip, then UpdatePlanExecutionStep(skip) -> executionState 'completedWithExceptions', stepStates {'OnCallSignOff': 'skipped'}, a stepSkipped event, listed under ListPlanExecutions(state=completedWithExceptions)
    actual:   AssertionError: the unapproved step left the execution in 'planExecutionTimedOut', expected 'pausedByFailedStep' before the skip
  check arc-region-switch.skip-not-started-step-rejected
    expected: ('pausedByFailedStep', {'FirstGate': 'failed', 'SecondGate': 'notStarted'}), then UpdatePlanExecutionStep(skip) on SecondGate refused with ValidationException 'Wait for the step to start before skipping' and no state change, then skipping FirstGate -> {'FirstGate': 'skipped', 'SecondGate': 'pendingApproval'} with executionState 'pendingManualApproval'
    actual:   AssertionError: after the first gate's timeout the execution read ('planExecutionTimedOut', {'FirstGate': 'canceled', 'SecondGate': 'notStarted'})
  (All three waited out the real one-minute timeoutMinutes window — the elapsed>=55s guard passed — so the divergence is the transition taken, not its timing.)

Cause 2 of 2 — GetPlanExecution not-found message:
  check arc-region-switch.get-plan-execution-unknown-execution-id-not-found
    expected: ('ResourceNotFoundException', 'Execution not found for execution: <hex part of the requested executionId>')
    actual:   ('ResourceNotFoundException', 'No execution us-west-2/3c6ef37200000000 found for plan arn:aws:arc-region-switch::000000000000:plan/sim2real-ars-a5b104d2653b:7wdrqp')
  check arc-region-switch.get-plan-execution-steps-follow-approvals-unissued-execution-rejected
    expected: ('ResourceNotFoundException', 'Execution not found for execution: ffffffffffffffff')
    actual:   ('ResourceNotFoundException', 'No execution us-west-2/ffffffffffffffff found for plan arn:aws:arc-region-switch::000000000000:plan/sim2real-pg-d85a632898e6:7wdrqp')
  (The ids, plan names and account above are per-run values; the requirement is the fixed text 'Execution not found for execution: ' plus the hex component of the requested executionId, with no plan ARN.)

## Steps to Reproduce
Cause 1 of 2:
1. CreatePlan with regions=[<other-region>, <current-region>], recoveryApproach=activePassive, primaryRegion=<other-region>, executionRole=<role assumable by arc-region-switch.amazonaws.com>, and one activate workflow whose single step is {name: 'AwaitSignOff', executionBlockType: 'ManualApproval', executionBlockConfiguration: {executionApprovalConfig: {approvalRole: <role>, timeoutMinutes: 1}}}.
2. StartPlanExecution(planArn, targetRegion=<current-region>, action=activate).
3. Poll GetPlanExecution until executionState leaves pending/inProgress/pendingManualApproval (about 60 s, the configured window). Observe executionState 'planExecutionTimedOut' with stepStates {'AwaitSignOff': 'canceled'} and an endTime set; AWS gives 'pausedByFailedStep' with {'AwaitSignOff': 'failed'} and no endTime.
4. ListPlanExecutionEvents: the emulator records executionFailed, not stepFailed + stepPausedByError.
5. UpdatePlanExecutionStep(planArn, executionId, stepName='AwaitSignOff', actionToTake='skip', comment='...'): on AWS the step becomes skipped and the execution ends completedWithExceptions with a stepSkipped event; on the emulator the execution is already terminal.
6. With a two-step plan (FirstGate timeoutMinutes=1, SecondGate timeoutMinutes=60), after FirstGate's window closes, UpdatePlanExecutionStep(skip) on SecondGate must be refused with ValidationException 'Wait for the step to start before skipping' and leave the execution unchanged.

Cause 2 of 2:
1. CreatePlan and StartPlanExecution as above; note the returned executionId, which has the form '<region>/<16 hex digits>'.
2. Call GetPlanExecution(planArn=<the plan>, executionId='<same region>/<a different 16-hex-digit value the plan never issued>').
3. Expect HTTP 404 ResourceNotFoundException with message 'Execution not found for execution: <that 16-hex-digit value>'. The emulator answers 'No execution <region>/<hex> found for plan <planArn>'.
