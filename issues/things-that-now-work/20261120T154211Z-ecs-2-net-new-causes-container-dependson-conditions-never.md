Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.container-depends-on-complete-admits-nonzero-exit-unlike-success ecs.container-depends-on-success-gates-dependent-start ecs.container-depends-on-healthy-waits-for-the-health-check-to-pass ecs.execute-command-without-enable-flag-rejected
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.container-depends-on-complete-admits-nonzero-exit-unlike-success|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: ecs.container-depends-on-success-gates-dependent-start|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: ecs.container-depends-on-healthy-waits-for-the-health-check-to-pass|_poll_until#11|Timeout|return last
Issue-Key: ecs.execute-command-without-enable-flag-rejected|run#19|AssertionError|assert control == _SUCCEEDED, (
Issue-Commit: a14965d6060559849ea419b74faa25e7836c4558

# 2 net-new causes: container dependsOn conditions never gate container start, and ExecuteCommand on an exec-enabled task answers TargetNotConnectedException

> _Note: this issue was identified against the code base at commit `a14965d6060559849ea419b74faa25e7836c4558`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 2 — `containerDefinitions[].dependsOn` is not honoured, so a task with any ordering condition never progresses.
AWS: Smithy `ContainerDependency.condition` (enum `ContainerCondition`: START, COMPLETE, SUCCESS, HEALTHY) is documented as: START "validates that a dependent container is started before permitting other containers to start"; COMPLETE "validates that a dependent container runs to completion (exits) before permitting other containers to start"; SUCCESS is "the same as COMPLETE, but it also requires that the container exits with a zero status"; HEALTHY "validates that the dependent container passes its Docker health check before permitting other containers to start". So under COMPLETE a dependent starts after a non-zero exit (and the task ends with the dependent's own exit code and `stoppedReason` EssentialContainerExited), under SUCCESS it does not start at all and has no `exitCode`, and under HEALTHY the dependent's start is held until the health check passes — measurably longer than the same task gated on START.
Emulator: none of these are observable. All three checks run the task and then wait on the container states/exit codes that the condition is supposed to produce, and each ends `Check timed out after 60.0s`: the containers never execute and the per-condition gating never happens, so COMPLETE vs SUCCESS vs HEALTHY vs START are indistinguishable.
Checks: ecs.container-depends-on-complete-admits-nonzero-exit-unlike-success, ecs.container-depends-on-success-gates-dependent-start, ecs.container-depends-on-healthy-waits-for-the-health-check-to-pass

Cause 2 of 2 — ExecuteCommand refuses a task that WAS launched with `enableExecuteCommand` and whose exec agent is reported connected.
AWS: Smithy ExecuteCommand's modeled errors are AccessDeniedException, ClientException, ClusterNotFoundException, InvalidParameterException, ServerException and TargetNotConnectedException (all HTTP 400 except AccessDenied 403 / Server 500). On a RUNNING task started with `enableExecuteCommand=true` whose `ExecuteCommandAgent` managed agent has reached CONNECTED, ExecuteCommand SUCCEEDS and returns the `Session` plus `clusterArn`/`containerArn`/`containerName`/`taskArn`. `TargetNotConnectedException` is reserved for a target that is genuinely not connected; `InvalidParameterException` is what AWS returns for the negative half of this check — a task launched WITHOUT `enableExecuteCommand`.
Emulator: the positive control is refused — `('TargetNotConnectedException', 'The execute command failed due to an internal error. Wait and try again.')` — where AWS opens a session, so the check's differential (reject without the flag, accept with it) collapses.
Checks: ecs.execute-command-without-enable-flag-rejected

Why: Cause 1 makes every multi-container task with an init/sidecar ordering contract behave as if the contract did not exist — a client cannot verify startup order, cannot observe the non-zero-exit vs zero-exit distinction between COMPLETE and SUCCESS, and any caller waiting on the gated container hangs. Cause 2 means exec is unreachable even when enabled, and because the refusal is `TargetNotConnectedException` rather than the `InvalidParameterException` AWS uses for a non-exec task, a client cannot distinguish "you forgot enableExecuteCommand" from "the agent is not up" and its retry logic chases the wrong condition.

Fix: Cause 1 belongs in the task-startup/container state machine — the component that starts containers must evaluate each `dependsOn` entry's condition against the depended-on container's start, exit, exit status and health-check result before starting the dependent, and must publish the resulting container states and exit codes. Cause 2 belongs in the ExecuteCommand handler's target-readiness check and its error classification: a task with `enableExecuteCommand=true` and a CONNECTED `ExecuteCommandAgent` must be served a session, and the refusal path must select `InvalidParameterException` for a task launched without the flag. Related open issues 20261105T143800Z-ecs-2-net-new-causes-containers-never-execute.md and 20261119T173202Z-ecs-5-net-new-causes-executecommand-never-opens.md describe neighbouring symptoms but carry none of these four checks' replay keys.

Traces: the four check ids listed per cause above.

## Evidence
Cause 1 — Expected: with COMPLETE, the dependent `app` starts after `init` exits non-zero and the task ends with `app`'s `exitCode` 42 and `stoppedReason` EssentialContainerExited; with SUCCESS on the same non-zero exit, `app` never starts and has no `exitCode`; with HEALTHY, `app`'s measured wait is >= the depended-on container's health-ready delay while an identical START control waits less. Actual: no container state or exit code ever materialises; all three checks end `Check timed out after 60.0s`.

Cause 2 — Expected: ExecuteCommand on the RUNNING exec-enabled task returns 200 with a `session` (and `clusterArn`/`containerArn`/`containerName`/`taskArn`); the paired negative on a task launched without `enableExecuteCommand` returns HTTP 400 `InvalidParameterException`. Actual: the positive call returns HTTP 400 `TargetNotConnectedException` with message `The execute command failed due to an internal error. Wait and try again.` — a fixed message emitted for the wrong condition.

## Steps to Reproduce
Cause 1: CreateCluster; RegisterTaskDefinition (FARGATE/awsvpc) with an `init` container and an `app` container whose `dependsOn` names `init` with condition COMPLETE, then a twin with SUCCESS, then a pair with HEALTHY vs START where the depended-on container only becomes healthy after a delay. RunTask each and poll DescribeTasks for the containers' `lastStatus`, `exitCode` and start ordering.

Cause 2: CreateCluster; create an SSM-capable task role; RunTask the task definition twice — once with `enableExecuteCommand=true`, once without. Wait for the exec-enabled task's `ExecuteCommandAgent` managed agent to report CONNECTED, then call ExecuteCommand on it (expect a session; got TargetNotConnectedException) and on the non-exec task (expect InvalidParameterException).
