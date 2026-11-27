Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.execute-command-blank-command-rejected ecs.execute-command-unknown-container-name-rejected ecs.list-tags-for-resource-refuses-a-stopped-task ecs.run-task-unassumable-execution-role-override-rejected ecs.run-task-windows-fargate-platform-refuses-firelens-task-definition-rejected ecs.stop-task-malformed-identifier-and-overlong-reason-rejected ecs.task-state-change-sequencing ecs.update-container-instance-invalid-status-fails
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.execute-command-blank-command-rejected|_refusal#21|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: ecs.execute-command-unknown-container-name-rejected|run#22|AssertionError|assert got == _NO_SUCH_CONTAINER, f"refusal for an absent container: {got!r}"
Issue-Key: ecs.list-tags-for-resource-refuses-a-stopped-task|run#21|AssertionError|raise AssertionError(f"ListTagsForResource still reads the stopped task's tags {leftover!r}")
Issue-Key: ecs.run-task-unassumable-execution-role-override-rejected|run#15|AssertionError|raise AssertionError(f"RunTask accepted a role that does not exist: {accepted!r}")
Issue-Key: ecs.run-task-windows-fargate-platform-refuses-firelens-task-definition-rejected|_refusal#34|AssertionError|raise AssertionError(f"expected the call to be refused, but it was accepted: {result!r}")
Issue-Key: ecs.stop-task-malformed-identifier-and-overlong-reason-rejected|run#35|AssertionError|assert got == ("InvalidParameterException", message), (
Issue-Key: ecs|SubmitTaskStateChange|InvalidParameterException
Issue-Key: ecs.update-container-instance-invalid-status-fails|run#11|AssertionError|assert error == expected, f"expected {expected!r} for status=REGISTERING, got {error!r}"
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# ecs: 8 net-new causes — ExecuteCommand/RunTask/ListTagsForResource/StopTask/SubmitTaskStateChange/UpdateContainerInstancesState refusals are missing, mis-coded or mis-worded

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteCommand

## Description
Eight independent net-new causes in the ECS control-plane refusal surface. Each is a separate work item; close all eight before submitting.

Cause 1 of 8: ExecuteCommand accepts a blank `command`.
AWS: ExecuteCommand's `command` is a required member (Smithy: ExecuteCommandRequest.command required; modeled errors include InvalidParameterException, httpError 400). A request whose command string is empty/blank is refused with HTTP 400 InvalidParameterException and the message `Command can not be blank.` — fixed text, no variable part.
Emulator: returns HTTP 200 with a full ExecuteCommandResponse (clusterArn, containerArn, containerName, interactive, session{sessionId,streamUrl,tokenValue}, taskArn) for the blank command.
Why: a client that relies on the API rejecting an empty command gets a usable session handle for a command that can never run, so bad input is detected only inside the session (or never), and negative-path code is never exercised.
Fix: input validation in the ExecuteCommand handler (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/execute_command.rs) before the session is minted.
Traces: check ecs.execute-command-blank-command-rejected.

Cause 2 of 8: ExecuteCommand's unknown-container refusal is worded wrongly.
AWS: for a `container` name that is not in the task, the refusal is HTTP 400 InvalidParameterException with the message `Unable to start session because the container doesn’t exist. Specify a valid container and try again.` (fixed text, including the U+2019 right single quote in “doesn’t”; no variable part).
Emulator: same code, wrong message — `The specified container was not found. Specify a valid container name and try again.`
Why: clients and test suites that match on the message text (the only way to distinguish ExecuteCommand's InvalidParameterException sub-cases, since the code is shared) mis-classify the failure.
Fix: the error message constructed by the ExecuteCommand handler's container lookup (src/ops/execute_command.rs).
Traces: check ecs.execute-command-unknown-container-name-rejected.

Cause 3 of 8: ListTagsForResource answers for a STOPPED task instead of refusing.
AWS: ListTagsForResource on a task ARN whose task has stopped is refused with HTTP 400 InvalidParameterException and the message `The specified task is stopped. Specify a running task and try again.` (fixed text). Tags of a still-running sibling task remain readable (Smithy: ListTagsForResource errors include InvalidParameterException, httpError 400; output.tags length 0..50).
Emulator: returns HTTP 200 with `tags: []` for the stopped task's ARN.
Why: a client cannot tell “this task stopped” from “this task has no tags”; tag-reconciliation logic keyed on the refusal silently treats a stopped task as an untagged live one.
Fix: the resource-state branch of the ListTagsForResource handler (src/ops/list_tags_for_resource.rs) for task ARNs.
Traces: check ecs.list-tags-for-resource-refuses-a-stopped-task.

Cause 4 of 8: RunTask accepts an overrides.executionRoleArn naming a role that does not exist.
AWS: RunTask with `overrides.executionRoleArn` pointing at an absent/unassumable role is refused with HTTP 400 ClientException and the message `ECS was unable to assume the role '<executionRoleArn>' that was provided for this task. Please verify that the role being passed has the proper trust relationship and permissions and that your IAM user has permissions to pass this role.` — the only variable part is the role ARN taken from the request; the rest is fixed text (Smithy: RunTask errors include ClientException, httpError 400).
Emulator: returns HTTP 200 and creates the task (lastStatus PROVISIONING, overrides echoing the absent executionRoleArn).
Why: an IAM mis-wiring that AWS surfaces synchronously at RunTask becomes an accepted task in the emulator, so clients never see the refusal they handle and the deployment appears healthy.
Fix: execution-role resolution/validation in the RunTask handler (src/ops/run_task.rs) before task state is created.
Traces: check ecs.run-task-unassumable-execution-role-override-rejected.

Cause 5 of 8: RunTask accepts a FireLens task definition on a Windows Fargate platform version.
AWS: when the requested platformVersion cannot satisfy a required task-definition capability (here `ecs.capability.firelens.fluentbit` on Windows platformVersion 1.0.0), RunTask is refused with HTTP 400 PlatformTaskDefinitionIncompatibilityException and the message `One or more of the requested capabilities are not supported.` (fixed text). Smithy: RunTask errors include PlatformTaskDefinitionIncompatibilityException — “The specified platform version doesn't satisfy the required capabilities of the task definition.”
Emulator: returns HTTP 200 and launches the task (log-router + app containers PENDING).
Why: capability/platform incompatibility is reported by AWS before anything runs; the emulator starts a task that cannot exist on real Fargate, so clients never exercise the incompatibility path.
Fix: platform-vs-requiredCapabilities validation in the RunTask handler (src/ops/run_task.rs).
Traces: check ecs.run-task-windows-fargate-platform-refuses-firelens-task-definition-rejected.

Cause 6 of 8: StopTask mis-classifies a malformed task identifier as not-found.
AWS: StopTask validates the shape of the task identifier before looking it up. A bare task id whose length is neither 32 nor 36 is refused with HTTP 400 InvalidParameterException and the message `taskId length should be one of [32,36]` (fixed text). Sibling malformed forms get their own fixed messages (`Invalid identifier: ARN ends with a separator`, etc.).
Emulator: answers HTTP 400 InvalidParameterException with `The referenced task was not found` for a 31-character id.
Why: the client cannot distinguish “you sent a malformed identifier” (never retryable) from “that task is gone” (benign/idempotent), so retry and cleanup logic takes the wrong branch.
Fix: identifier-shape validation ordering in the StopTask handler (src/ops/stop_task.rs), before the state lookup.
Traces: check ecs.stop-task-malformed-identifier-and-overlong-reason-rejected.

Cause 7 of 8: SubmitTaskStateChange refuses a stale out-of-order RUNNING signal instead of absorbing it.
AWS: SubmitTaskStateChange is the agent-facing signal API; a stale/duplicate signal is a safe no-op. Submitting status=RUNNING for a task already STOPPED answers HTTP 200 with an acknowledgment and leaves lastStatus STOPPED (it does not resurrect the task); a duplicate STOPPED is likewise a 200 no-op (recorded behavior; Smithy models only output.acknowledgment plus the standard error set).
Emulator: raises HTTP 400 InvalidParameterException `The status RUNNING is not a valid status for a task in the STOPPED status.`, which aborts the caller.
Why: an agent (or any replayed/duplicated signal) sees a hard client error for a situation AWS tolerates, so at-least-once signal delivery becomes a failure instead of an idempotent no-op.
Fix: the state-transition guard in the SubmitTaskStateChange handler (src/ops/submit_task_state_change.rs) — ignore non-advancing transitions rather than erroring.
Traces: check ecs.task-state-change-sequencing.

Cause 8 of 8: UpdateContainerInstancesState's invalid-status message drops its subject.
AWS: a target status outside {ACTIVE, DRAINING} (REGISTERING is a reported ContainerInstanceStatus value but never an accepted target — Smithy: ContainerInstanceStatus = ACTIVE, DRAINING, REGISTERING, DEREGISTERING, REGISTRATION_FAILED) is refused with HTTP 400 InvalidParameterException and the message `Container instances status should be one of [ACTIVE,DRAINING]` (fixed text; the bracketed list is the accepted-target set).
Emulator: same code, message `status should be one of [ACTIVE,DRAINING]` — the `Container instances ` prefix is missing.
Why: callers matching the documented message text to attribute the refusal to the container-instance status field mis-handle it.
Fix: the message built by the status validation in the UpdateContainerInstancesState handler (src/ops/update_container_instances_state.rs).
Traces: check ecs.update-container-instance-invalid-status-fails.

## Evidence
Cause 1 — expected (InvalidParameterException, 'Command can not be blank.'); got HTTP 200 ExecuteCommandResponse with a session (sessionId/streamUrl/tokenValue).
Cause 2 — expected ('InvalidParameterException', 'Unable to start session because the container doesn’t exist. Specify a valid container and try again.'); got ('InvalidParameterException', 'The specified container was not found. Specify a valid container name and try again.').
Cause 3 — expected ('InvalidParameterException', 'The specified task is stopped. Specify a running task and try again.'); got HTTP 200 with tags {} (AssertionError: ListTagsForResource still reads the stopped task's tags {}).
Cause 4 — expected ('ClientException', "ECS was unable to assume the role '<arn>' that was provided for this task. Please verify that the role being passed has the proper trust relationship and permissions and that your IAM user has permissions to pass this role."); got HTTP 200, task created with overrides.executionRoleArn set to the absent role (AssertionError: RunTask accepted a role that does not exist).
Cause 5 — expected ('PlatformTaskDefinitionIncompatibilityException', 'One or more of the requested capabilities are not supported.'); got HTTP 200 with tasks[] containing log-router and app containers in PENDING.
Cause 6 — expected ('InvalidParameterException', 'taskId length should be one of [32,36]'); got ('InvalidParameterException', 'The referenced task was not found').
Cause 7 — expected HTTP 200 acknowledgment and lastStatus unchanged at STOPPED; got botocore.errorfactory.InvalidParameterException: 'The status RUNNING is not a valid status for a task in the STOPPED status.' raised by SubmitTaskStateChange.
Cause 8 — expected ('InvalidParameterException', 'Container instances status should be one of [ACTIVE,DRAINING]'); got ('InvalidParameterException', 'status should be one of [ACTIVE,DRAINING]').

## Steps to Reproduce
Replay the behavioral suite and re-run the listed checks:
1. ecs.execute-command-blank-command-rejected — RunTask an exec-enabled task, then ExecuteCommand(cluster, task, container='shell', interactive=True, command='').
2. ecs.execute-command-unknown-container-name-rejected — ExecuteCommand naming a container not present in the task definition.
3. ecs.list-tags-for-resource-refuses-a-stopped-task — RunTask count=2 with tags, StopTask one, wait tasks_stopped, then ListTagsForResource on the stopped task's ARN (and on the running sibling).
4. ecs.run-task-unassumable-execution-role-override-rejected — RunTask with overrides.executionRoleArn = an IAM role ARN that was never created.
5. ecs.run-task-windows-fargate-platform-refuses-firelens-task-definition-rejected — register a Windows FireLens task definition, then RunTask launchType=FARGATE platformVersion='1.0.0'.
6. ecs.stop-task-malformed-identifier-and-overlong-reason-rejected — StopTask with the first 31 characters of a real task id.
7. ecs.task-state-change-sequencing — let a short-lived task reach STOPPED, then SubmitTaskStateChange(cluster, task, status='RUNNING', reason='stale-replay') and again with status='STOPPED'.
8. ecs.update-container-instance-invalid-status-fails — UpdateContainerInstancesState(containerInstances=[arn], status='REGISTERING').
