Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__ssm__stepfunctions.maintenance-window-task-starts-ssm-prefixed-state-machine-under-managed-window-policy iam__ssm__stepfunctions.register-step-functions-task-without-pass-role-rejected
Issue-Key: iam__ssm__stepfunctions.maintenance-window-task-starts-ssm-prefixed-state-machine-under-managed-window-policy|_poll#19|Timeout|time.sleep(pause)
Issue-Key: iam__ssm__stepfunctions.register-step-functions-task-without-pass-role-rejected|_refused_with#24|AssertionError|raise AssertionError(f"the registration was expected to be refused, it returned {accepted!r}")
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# iam__ssm__stepfunctions: 2 causes — a window's ssm-prefixed Step Functions task never starts, and the task registers without iam:PassRole

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTaskWithMaintenanceWindow

## Description
Cause 1 of 2: a maintenance-window Step Functions task running under the managed window policy never starts the state machine.
AWS: a due window whose service role carries the managed maintenance-window policy starts the `ssm`-prefixed state machine its task names, and the window execution and the state-machine execution are both observable.
Emulator: the scenario makes no progress and is cut at the harness cap, with no window execution and no state-machine execution.
Why: the documented 'managed policy is sufficient for an ssm-prefixed state machine' behaviour cannot be relied on, and nothing the window was scheduled to run runs.
Fix: the maintenance-window scheduler's StepFunctions task invoker and the role it assumes (services/service-com-amazonaws-ssm-amazonssm/src/maintenance_window/…).
Traces: iam__ssm__stepfunctions.maintenance-window-task-starts-ssm-prefixed-state-machine-under-managed-window-policy.

Cause 2 of 2: RegisterTaskWithMaintenanceWindow accepts a Step Functions task when the caller lacks iam:PassRole on the service role.
AWS: the registration is refused with AccessDeniedException naming iam:PassRole and the role; no task is created.
Emulator: returns 200 with a WindowTaskId.
Why: a caller can attach a role it has no right to pass — the refusal that bounds privilege escalation is missing.
Fix: the PassRole check in ops/register_task_with_maintenance_window.rs.
Traces: iam__ssm__stepfunctions.register-step-functions-task-without-pass-role-rejected.

## Evidence
Cause 1: 'Check timed out after 60.0s' with no window execution observed.
Cause 2: 'the registration was expected to be refused, it returned {WindowTaskId: 3ec3fd0c-…, HTTPStatusCode: 200}'.

## Steps to Reproduce
1. Create a state machine named with the ssm- prefix, a window service role holding only AmazonSSMMaintenanceWindowRole, a due window and a target; register the STEP_FUNCTIONS task and poll for the window and state-machine executions.
2. As a principal with no iam:PassRole on that service role, call RegisterTaskWithMaintenanceWindow for the same task.
