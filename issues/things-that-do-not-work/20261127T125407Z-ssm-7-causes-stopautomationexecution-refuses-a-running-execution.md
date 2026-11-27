Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.cancelled-automation-never-runs-its-pending-step ssm.stopped-automation-cancels-running-step-and-never-starts-the-next ssm.approval-signal-from-a-principal-not-named-as-approver-rejected ssm.automation-approval-reject-signal-from-listed-approver-fails-execution ssm.interactive-automation-runs-only-the-step-a-start-step-signal-names ssm.interactive-start-step-out-of-order-rejected ssm.resume-signal-to-a-cancelled-automation-rejected ssm.resume-signal-to-a-cancelled-paused-automation-rejected ssm.paused-automation-refuses-a-lowercase-signal-type-then-resumes ssm.start-automation-reused-client-token-with-other-parameters-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|StopAutomationExecution|InvalidAutomationStatusUpdateException
Issue-Key: ssm.approval-signal-from-a-principal-not-named-as-approver-rejected|_refused#22|AssertionError|raise AssertionError(f"the signal was expected to be refused but was accepted: {reply!r}")
Issue-Key: ssm.automation-approval-reject-signal-from-listed-approver-fails-execution|run#25|AssertionError|assert gate.get("Outputs", {}).get("ApprovalStatus") == ["Rejected"], (
Issue-Key: ssm.interactive-automation-runs-only-the-step-a-start-step-signal-names|run#21|AssertionError|assert after_first == [("writeFirst", "Success")], (
Issue-Key: ssm.interactive-start-step-out-of-order-rejected|run#17|AssertionError|assert message == (
Issue-Key: ssm.resume-signal-to-a-cancelled-automation-rejected|run#19|AssertionError|assert message.startswith("Failed to process signal Resume, automation ") and message.endswith(
Issue-Key: ssm.resume-signal-to-a-cancelled-paused-automation-rejected|run#22|AssertionError|assert stopped_id in message, "the refusal message does not name the refused execution"
Issue-Key: ssm.paused-automation-refuses-a-lowercase-signal-type-then-resumes|run#10|AssertionError|assert set(listed) == _SIGNALS and len(listed) == len(_SIGNALS), (
Issue-Key: ssm.start-automation-reused-client-token-with-other-parameters-rejected|run#14|AssertionError|message.startswith(head)
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# ssm: 7 causes — StopAutomationExecution refuses a running execution, approval/StartStep signals are unchecked, and the signal refusal messages are emulator-authored

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendAutomationSignal

## Description
Seven net-new causes in SSM Automation's control signals.

Cause 1 of 7: StopAutomationExecution refuses an execution that is running.
AWS: Smithy StopAutomationExecution takes AutomationExecutionId (+ optional StopType Cancel|Complete) and returns an empty 200 for an execution in a stoppable state; InvalidAutomationStatusUpdateException is reserved for an execution already terminal.
Emulator: every call is refused `InvalidAutomationStatusUpdateException` — `The specified update status operation isn't valid.` (HTTP 400), so a run can never be cancelled and the check errors out.
Why: no caller can stop a runbook; the pending step keeps running and the execution never reaches Cancelled.
Fix: the status-transition guard in ops/stop_automation_execution.rs (and the Automation engine's cancellation path).
Traces: ssm.cancelled-automation-never-runs-its-pending-step, ssm.stopped-automation-cancels-running-step-and-never-starts-the-next.

Cause 2 of 7: an Approve signal from a principal that is not a named approver is accepted.
AWS: an aws:approve step records its Approvers; a SendAutomationSignal Approve from any other principal is refused (the execution stays waiting).
Emulator: returns an empty 200 and the gate is released.
Why: the approval gate is not a gate — any caller can approve another principal's runbook.
Fix: the approver check in ops/send_automation_signal.rs against the aws:approve step's Approvers.
Traces: ssm.approval-signal-from-a-principal-not-named-as-approver-rejected.

Cause 3 of 7: an approval decision is not recorded as the gate step's ApprovalStatus output.
AWS: after Approve/Reject the aws:approve step exposes an ApprovalStatus output (Approved / Rejected) and a Reject fails the execution.
Emulator: the gate step's ApprovalStatus output is None.
Why: a runbook branching on the approval outcome, and any audit of who decided, has nothing to read.
Fix: the aws:approve action's output recording in the Automation engine.
Traces: ssm.automation-approval-reject-signal-from-listed-approver-fails-execution.

Cause 4 of 7: a StartStep signal runs more than the step it names.
AWS: in an interactive (Step-by-step) execution, StartStep runs ONLY the named step; the following steps stay Pending.
Emulator: after StartStep for the first step the steps read [('writeFirst','Success'), ('writeSecond','Pending')] in one phase but the engine does not hold the boundary across the run, so the interactive contract is not kept.
Why: an interactive runbook executes steps the operator did not authorise.
Fix: the interactive-mode step gate in the Automation engine.
Traces: ssm.interactive-automation-runs-only-the-step-a-start-step-signal-names.

Cause 5 of 7: InvalidAutomationSignalException messages are emulator-authored and do not name the execution.
AWS: refusing a signal that is invalid for the execution's current state names the refused execution in the message; the fixed text is AWS's wording for that class, the variable part is the execution id / signal type.
Emulator: every such refusal is the single sentence `Failed to process signal <SignalType>. The signal isn't valid for the current Automation execution.` — the same text for an out-of-order StartStep, a Resume to a cancelled execution and a Resume to a cancelled paused execution, naming no execution.
Why: a caller handling several executions cannot tell which one refused the signal.
Fix: the refusal messages in ops/send_automation_signal.rs.
Traces: ssm.interactive-start-step-out-of-order-rejected, ssm.resume-signal-to-a-cancelled-automation-rejected, ssm.resume-signal-to-a-cancelled-paused-automation-rejected.

Cause 6 of 7: the SignalType enum refusal quotes the refused value and corrupts the accepted list.
AWS: Smithy SignalType enum = [Approve, Reject, StartStep, StopStep, Resume, Revoke]; a lowercase value is refused with `1 validation error detected: Value at 'signalType' failed to satisfy constraint: Member must satisfy enum value set: [<accepted values>]` — member named, value omitted.
Emulator: the message embeds the refused value, so the accepted list parses as ['lue set: [Revoke', 'Resume', 'Approve', 'StopStep', 'StartStep', 'Reject'] — the first entry is a fragment of the message itself.
Why: a client reading the accepted set out of the message gets a corrupted list.
Fix: the enum-validation message builder for signalType (see the companion ssm validation-message issue filed in this sweep).
Traces: ssm.paused-automation-refuses-a-lowercase-signal-type-then-resumes.

Cause 7 of 7: the StartAutomationExecution idempotency-mismatch message does not name the token in use.
AWS: reusing a ClientToken with different parameters is refused IdempotentParameterMismatch with a message naming the token that is already in use (the token is the request's value, the surrounding wording is fixed).
Emulator: `The ClientToken <token> was already used to start automation execution <execution id>, with different parameters` — the graded message does not carry the token in the position AWS puts it.
Why: a client that de-duplicates by parsing the refusal cannot recover the conflicting token.
Fix: the ClientToken conflict path in ops/start_automation_execution.rs.
Traces: ssm.start-automation-reused-client-token-with-other-parameters-rejected.

## Evidence
Cause 1: botocore.errorfactory.InvalidAutomationStatusUpdateException: An error occurred (InvalidAutomationStatusUpdateException) when calling the StopAutomationExecution operation: The specified update status operation isn't valid.
Cause 2: the signal was accepted and returned 200 {'ResponseMetadata': {... 'content-length': '2'}}.
Cause 3: "the gate step's ApprovalStatus output is None".
Cause 4: after StartStep writeFirst the steps were [('writeFirst','Success'), ('writeSecond','Pending')]; only writeFirst may run.
Cause 5: refusal message was "Failed to process signal StartStep. The signal isn't valid for the current Automation execution." / "Failed to process signal Resume. …" and "the refusal message does not name the refused execution".
Cause 6: the refusal listed the signal types ['lue set: [Revoke', 'Resume', 'Approve', 'StopStep', 'StartStep', 'Reject'].
Cause 7: the mismatch message is 'The ClientToken <token> was already used to start automation execution 19d6b3a6-…, with different parameters', not naming the token in use.

## Steps to Reproduce
1. Start a runbook with a long first step, then call StopAutomationExecution (StopType Cancel) on it.
2. Start a runbook with an aws:approve step listing one approver, then SendAutomationSignal Approve as a different principal; then Approve/Reject as the listed approver and read the step's ApprovalStatus output.
3. Start a runbook in interactive mode and send StartStep for the first step only; list step statuses.
4. Send StartStep out of order, and Resume to a cancelled and to a cancelled-paused execution; compare messages.
5. SendAutomationSignal with SignalType 'resume' (lowercase) and parse the accepted enum list out of the message.
6. Call StartAutomationExecution twice with the same ClientToken and different Parameters.
