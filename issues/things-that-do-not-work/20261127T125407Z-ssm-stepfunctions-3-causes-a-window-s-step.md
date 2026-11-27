Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm__stepfunctions.maintenance-window-step-functions-task-resolves-pseudo-parameters ssm__stepfunctions.step-functions-task-non-json-input-rejected-and-never-runs ssm__stepfunctions.step-functions-task-with-lambda-invocation-block-rejected
Issue-Key: ssm__stepfunctions.maintenance-window-step-functions-task-resolves-pseudo-parameters|_wait_for#21|AssertionError|raise AssertionError(f"{what} did not happen within {budget:.0f}s")
Issue-Key: ssm__stepfunctions.step-functions-task-non-json-input-rejected-and-never-runs|_refusal#21|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {result!r}")
Issue-Key: ssm__stepfunctions.step-functions-task-with-lambda-invocation-block-rejected|_rejection#16|AssertionError|raise AssertionError(f"the registration should have been refused; AWS returned {answer!r}")
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# ssm__stepfunctions: 3 causes — a window's Step Functions task never runs, and RegisterTaskWithMaintenanceWindow accepts a non-JSON input and a Lambda invocation block

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTaskWithMaintenanceWindow

## Description
Three net-new causes where maintenance windows drive Step Functions.

Cause 1 of 3: a registered Step Functions task never produces a finished window execution.
AWS: a due maintenance window starts the state machine named by its StepFunctions task (resolving pseudo parameters such as {{WINDOW_EXECUTION_ID}} / {{TARGET_ID}} in the task's Input) and the window execution finishes; DescribeMaintenanceWindowExecutions then lists it.
Emulator: no finished window execution appears within the poll window, so neither the task invocation nor the resolved input can be read.
Why: the whole maintenance-window→Step Functions integration is unobservable and nothing a window was scheduled to run ever runs.
Fix: the maintenance-window scheduler and its StepFunctions task invoker (see the companion ssm maintenance-window issue filed in this sweep).
Traces: ssm__stepfunctions.maintenance-window-step-functions-task-resolves-pseudo-parameters.

Cause 2 of 3: a StepFunctions task whose Input is not valid JSON is accepted.
AWS: RegisterTaskWithMaintenanceWindow validates TaskInvocationParameters.StepFunctions.Input as JSON and refuses a non-JSON body (HTTP 400); no task is registered and nothing runs.
Emulator: returns 200 with a WindowTaskId.
Why: the window carries a task that can never start a state machine, and the caller is told the registration succeeded.
Fix: the TaskInvocationParameters validator in ops/register_task_with_maintenance_window.rs.
Traces: ssm__stepfunctions.step-functions-task-non-json-input-rejected-and-never-runs.

Cause 3 of 3: a StepFunctions task registered with a Lambda invocation block is accepted.
AWS: TaskInvocationParameters must carry the block matching the task type; a StepFunctions task that also carries a Lambda block is refused (HTTP 400).
Emulator: returns 200 with a WindowTaskId.
Why: a mis-typed registration is stored, so the window's behaviour diverges from the template the caller believes was rejected.
Fix: the task-type/invocation-parameter cross-check in the same handler.
Traces: ssm__stepfunctions.step-functions-task-with-lambda-invocation-block-rejected.

## Evidence
Cause 1: 'a finished window execution did not happen within 240s'.
Cause 2: 'the call was expected to be refused but returned {WindowTaskId: e058b8a7-…, HTTPStatusCode: 200}'.
Cause 3: 'the registration should have been refused; AWS returned {WindowTaskId: 9255bbfb-…, HTTPStatusCode: 200}'.

## Steps to Reproduce
1. Create a state machine, a due maintenance window and a target, register a StepFunctions task whose Input uses {{WINDOW_EXECUTION_ID}}, then poll DescribeMaintenanceWindowExecutions / the state machine's executions.
2. RegisterTaskWithMaintenanceWindow with TaskInvocationParameters.StepFunctions.Input set to a non-JSON string.
3. RegisterTaskWithMaintenanceWindow for a STEP_FUNCTIONS task while also supplying a Lambda invocation block.
