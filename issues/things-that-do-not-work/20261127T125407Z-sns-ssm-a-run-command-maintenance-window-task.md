Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__ssm.run-command-task-notification-without-its-own-role-rejected
Issue-Key: sns__ssm.run-command-task-notification-without-its-own-role-rejected|_refused#18|AssertionError|raise AssertionError(f"expected a refusal, but the call was accepted: {accepted!r}")
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# sns__ssm: a run-command maintenance-window task with a NotificationConfig is registered without its own ServiceRoleArn

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTaskWithMaintenanceWindow

## Description
Cause 1 of 1: RegisterTaskWithMaintenanceWindow accepts a RUN_COMMAND task that carries a NotificationConfig but no role of its own.
AWS: publishing command notifications requires a role; a run-command task whose TaskInvocationParameters carry a NotificationConfig while the task supplies no ServiceRoleArn is refused (HTTP 400) and no task is registered.
Emulator: returns 200 with a WindowTaskId, so the window holds a task whose notifications can never be published.
Why: the caller believes notifications are wired; at run time nothing can publish them, and the refusal that would have caught the missing role never happens.
Fix: the NotificationConfig/ServiceRoleArn cross-check in the RegisterTaskWithMaintenanceWindow handler (services/service-com-amazonaws-ssm-amazonssm/src/ops/register_task_with_maintenance_window.rs).
Traces: sns__ssm.run-command-task-notification-without-its-own-role-rejected.

## Evidence
'expected a refusal, but the call was accepted: {WindowTaskId: 1cfdbfd6-0a0a-40e9-8def-61a330b9be57, HTTPStatusCode: 200, content-length: 55}'.

## Steps to Reproduce
Create a maintenance window and a target, then RegisterTaskWithMaintenanceWindow with TaskType RUN_COMMAND, TaskInvocationParameters.RunCommand.NotificationConfig set to an SNS topic, and no ServiceRoleArn on the task. AWS refuses; the emulator returns a WindowTaskId.
