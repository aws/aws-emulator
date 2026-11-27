Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.automation-assert-property-mismatch-lists-failed-with-its-message ssm.automation-failed-assertion-routes-to-its-on-failure-step-and-the-run-still-fails ssm.automation-step-timeout-ends-critical-run-timed-out-and-noncritical-run-continues ssm.automation-property-wait-step-holds-in-progress-until-released ssm.automation-continue-on-failure-succeeds-unless-the-step-is-marked-critical ssm.automation-loop-iteration-failure-continues-and-execution-succeeds ssm.automation-loop-left-by-an-inner-branch-reports-exited ssm.automation-refused-api-step-reports-verification-failure-details ssm.automation-api-step-failure-reports-the-api-error-and-skips-later-steps ssm.automation-step-past-its-timeout-ends-the-execution-timed-out ssm.rate-control-automation-with-a-timed-out-child-ends-failed ssm.automation-loop-max-iterations-above-100-rejected ssm.automation-parameter-outside-allowed-values-rejected ssm.ssm-start-automation-refuses-values-outside-the-document-parameter-constraints ssm.rate-control-target-parameter-the-runbook-lacks-rejected ssm.ssm-automation-parameter-default-resolves-a-parameter-store-reference
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.automation-assert-property-mismatch-lists-failed-with-its-message|run#10|AssertionError|assert ok_run["AutomationExecutionStatus"] == "Success", (
Issue-Key: ssm.automation-failed-assertion-routes-to-its-on-failure-step-and-the-run-still-fails|run#12|AssertionError|assert passed["AutomationExecutionStatus"] == "Success", (
Issue-Key: ssm.automation-step-timeout-ends-critical-run-timed-out-and-noncritical-run-continues|run#13|AssertionError|assert strict["AutomationExecutionStatus"] == "TimedOut", (
Issue-Key: ssm.automation-property-wait-step-holds-in-progress-until-released|run#13|AssertionError|assert gate_status in (None, "Pending"), (
Issue-Key: ssm.automation-continue-on-failure-succeeds-unless-the-step-is-marked-critical|run#10|AssertionError|assert lenient_status == "Success", (
Issue-Key: ssm.automation-loop-iteration-failure-continues-and-execution-succeeds|run#8|AssertionError|assert run_record["AutomationExecutionStatus"] == "Success", (
Issue-Key: ssm.automation-loop-left-by-an-inner-branch-reports-exited|run#14|AssertionError|assert status == "Success", f"the loop with no early jump must reach Success; got {status!r}"
Issue-Key: ssm.automation-refused-api-step-reports-verification-failure-details|run#23|AssertionError|assert step.get("FailureDetails") == expected_details, (
Issue-Key: ssm.automation-api-step-failure-reports-the-api-error-and-skips-later-steps|run#25|AssertionError|assert details == {
Issue-Key: ssm.automation-step-past-its-timeout-ends-the-execution-timed-out|_settle#11|AssertionError|raise AssertionError(
Issue-Key: ssm.rate-control-automation-with-a-timed-out-child-ends-failed|_settle#8|AssertionError|raise AssertionError(f"parent execution not terminal at the deadline: {rows!r}")
Issue-Key: ssm.automation-loop-max-iterations-above-100-rejected|_refusal#10|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {got!r}")
Issue-Key: ssm.automation-parameter-outside-allowed-values-rejected|run#10|AssertionError|assert got == (
Issue-Key: ssm|StartAutomationExecution|InvalidAutomationExecutionParametersException
Issue-Key: ssm.rate-control-target-parameter-the-runbook-lacks-rejected|run#6|AssertionError|assert (code, message) == (
Issue-Key: ssm.ssm-automation-parameter-default-resolves-a-parameter-store-reference|run#10|AssertionError|assert ex["AutomationExecutionStatus"] == "Success", (
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# ssm: 7 causes — the Automation engine does not run aws:assert/aws:waitFor steps, ignores onFailure/isCritical, mis-reports FailureDetails, leaves executions Pending, and accepts an over-cap loop

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAutomationExecution

## Description
Seven net-new causes in the SSM Automation execution engine.

Cause 1 of 7: the aws:assertAwsResourceProperty and aws:waitForAwsResourceProperty actions are not executed at all.
AWS: these actions read a property through the service API and either assert its value (step Success when the held value is among DesiredValues, Failed with the property mismatch message otherwise) or poll until it matches. Recorded behavior shows the step running and the execution reaching Success for a matching value.
Emulator: the execution ends `Failed` with FailureMessage `The aws:assertAwsResourceProperty action of step <step> runs where this emulator does not reach, so nothing ran for this step` (same text for aws:waitForAwsResourceProperty). The fixed text is that sentence; the only variable parts are the action name and the step name.
Why: every runbook that gates on a resource property fails, so no assertion or wait-based runbook can be modelled.
Fix: the Automation action dispatcher and these two action implementations (services/service-com-amazonaws-ssm-amazonssm/src/automation/…).
Traces: ssm.automation-assert-property-mismatch-lists-failed-with-its-message, ssm.automation-failed-assertion-routes-to-its-on-failure-step-and-the-run-still-fails, ssm.automation-step-timeout-ends-critical-run-timed-out-and-noncritical-run-continues, ssm.automation-property-wait-step-holds-in-progress-until-released.

Cause 2 of 7: onFailure Continue / isCritical is not honoured when deciding the execution's status.
AWS: a step whose onFailure is `Continue` is non-critical by default — the execution continues and ends `Success`; only a step marked `isCritical: true` (or onFailure Abort) makes the execution `Failed`. A failing iteration inside aws:loop likewise leaves the run Success when the loop continues past it.
Emulator: the execution ends `Failed` in all three shapes.
Why: a client reading AutomationExecutionStatus sees failure for a runbook AWS reports as Success, so retry/alarm logic fires wrongly.
Fix: the step-outcome-to-execution-status reducer in the Automation engine.
Traces: ssm.automation-continue-on-failure-succeeds-unless-the-step-is-marked-critical, ssm.automation-loop-iteration-failure-continues-and-execution-succeeds, ssm.automation-loop-left-by-an-inner-branch-reports-exited.

Cause 3 of 7: a refused aws:executeAwsApi step reports the wrong FailureDetails.
AWS: when the called API refuses the step, the step's FailureDetails are `{'FailureStage': 'Invocation', 'FailureType': 'Verification', 'Details': {'VerificationErrorMessage': ['<the SDK error text>']}}` — the stage/type/key names are fixed text, the error text comes from the refused call.
Emulator: `{'FailureStage': 'Execution', 'FailureType': 'Action'}` with no Details, so the API error is nowhere in the response.
Why: an operator or automation reading FailureDetails cannot tell which API refused the step or why.
Fix: the aws:executeAwsApi action's failure mapping and the StepExecution serializer.
Traces: ssm.automation-refused-api-step-reports-verification-failure-details, ssm.automation-api-step-failure-reports-the-api-error-and-skips-later-steps.

Cause 4 of 7: a started execution never leaves Pending.
AWS: StartAutomationExecution returns an id and the execution moves to InProgress and then to a terminal status; a step that outlives its timeoutSeconds ends the execution `TimedOut`, and a rate-control parent with a timed-out child ends `Failed`.
Emulator: GetAutomationExecution/DescribeAutomationExecutions still report `AutomationExecutionStatus: 'Pending'` at the check deadline, with no step ever started.
Why: nothing downstream of a runbook can ever be observed; callers poll forever.
Fix: the Automation scheduler that promotes a Pending execution (and rate-control children) to InProgress.
Traces: ssm.automation-step-past-its-timeout-ends-the-execution-timed-out, ssm.rate-control-automation-with-a-timed-out-child-ends-failed.

Cause 5 of 7: CreateDocument accepts an aws:loop step whose maxIterations exceeds the service cap.
AWS: a runbook declaring maxIterations above 100 is refused at CreateDocument time.
Emulator: returns 200 with a full DocumentDescription (Status 'Creating', DocumentType 'Automation').
Why: a runbook AWS would reject is stored and can be started, so the cap is never surfaced.
Fix: the schemaVersion 0.3 document validator in CreateDocument (ops/create_document.rs).
Traces: ssm.automation-loop-max-iterations-above-100-rejected.

Cause 6 of 7: InvalidAutomationExecutionParametersException messages are emulator-authored.
AWS: a value outside a parameter's allowedValues is refused with `Parameter "<name>" has value "<value>" not found in [<allowed values>].` — fixed text `Parameter "`, `" has value "`, `" not found in [`, `].`, with the parameter name, the submitted value and the document's allowed list filled in.
Emulator: `Document <document name> doesn't support the values supplied for parameter <name>` (and, for an undeclared TargetParameterName, `Document <name> doesn't support a parameter named <param>`), naming neither the value nor the allowed set.
Why: the caller cannot see which value was refused or what the document accepts.
Fix: the parameter-resolution validator shared by StartAutomationExecution and rate-control targeting.
Traces: ssm.automation-parameter-outside-allowed-values-rejected, ssm.ssm-start-automation-refuses-values-outside-the-document-parameter-constraints, ssm.rate-control-target-parameter-the-runbook-lacks-rejected.

Cause 7 of 7: an Automation parameter default holding an `{{ssm:<name>}}` reference is not resolved before the step runs.
AWS: SSM resolves `{{ssm:<name>}}` in a runbook parameter default to the parameter's value, so the step writes the resolved value.
Emulator: the literal `{{...}}` is passed through to the step, whose PutParameter is then refused — the execution ends `Failed` with `An error occurred (ValidationException) when calling the PutParameter operation: Value carries a value wrapped in double brackets: a parameter value can't reference or nest another parameter`.
Why: documented Parameter Store references in runbooks are inert, and the failure surfaces as an unrelated PutParameter refusal.
Fix: the pre-execution parameter resolution step of the Automation engine.
Traces: ssm.ssm-automation-parameter-default-resolves-a-parameter-store-reference.

## Evidence
Cause 1: AutomationExecutionStatus 'Failed', FailureMessage 'The aws:assertAwsResourceProperty action of step confirmValue runs where this emulator does not reach, so nothing ran for this step'; expected Success for a matching DesiredValue. For the timeout check: 'The aws:waitForAwsResourceProperty action of step outlast runs where this emulator does not reach…', expected the execution TimedOut.
Cause 2: 'a failed step that continues is non-critical by default, so the run must succeed; got Failed'; 'a failure the runbook continues past should leave the run Success, got Failed'; 'the loop with no early jump must reach Success; got Failed'.
Cause 3: expected FailureDetails {'FailureStage': 'Invocation', 'FailureType': 'Verification', 'Details': {'VerificationErrorMessage': ["An error occurred (ParameterNotFound) when calling the GetParameter operation: "]}}; got {'FailureStage': 'Execution', 'FailureType': 'Action'}.
Cause 4: last read AutomationExecutionStatus 'Pending' with no StepExecutions; rate-control parent listed 'Pending' at the deadline with ResolvedTargets ['PT1S','PT60S'].
Cause 5: CreateDocument answered 200 with DocumentDescription for the over-cap loop runbook.
Cause 6: got ('InvalidAutomationExecutionParametersException', "Document sim2real-behav-auto-<id> doesn't support the values supplied for parameter env"); expected ('InvalidAutomationExecutionParametersException', 'Parameter "env" has value "qa" not found in [dev, prod].').
Cause 7: execution Failed with the PutParameter double-brackets ValidationException instead of writing the resolved value.

## Steps to Reproduce
1. Create a 0.3 runbook with an aws:assertAwsResourceProperty step whose DesiredValues include the held value, start it, and read AutomationExecutionStatus; repeat with aws:waitForAwsResourceProperty.
2. Create a runbook with a failing step whose onFailure is 'Continue' (not marked critical) and start it; repeat inside an aws:loop.
3. Create a runbook with an aws:executeAwsApi step calling GetParameter on a missing parameter; read the step's FailureDetails.
4. Start a runbook whose step outlives timeoutSeconds, and a rate-control runbook with two ParameterValues targets; poll GetAutomationExecution.
5. CreateDocument for a runbook whose aws:loop declares maxIterations 101.
6. StartAutomationExecution with a parameter value outside allowedValues, and with a TargetParameterName the runbook does not declare.
7. Start a runbook whose parameter default is '{{ssm:<existing parameter>}}' and whose step writes it.
