Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.container-depends-on-complete-admits-nonzero-exit-unlike-success ecs.container-depends-on-healthy-waits-for-the-health-check-to-pass ecs.health-check-start-period-suppresses-unhealthy-then-expires ecs.exec-interactive-stdout-round-trip ecs.execute-command-session-credentials-round-trip ecs.deep-ecs-3-deploy-fail-metric-alarm-rolls-back-to-rev1-quiet-twin-completes ecs.service-cpu-utilization-metric-published ecs.task-protection-enforced-during-scale-in ecs.task-state-change-ordering-enforced-rejected
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.container-depends-on-complete-admits-nonzero-exit-unlike-success|run#20|AssertionError|assert control.get("stopCode") == "TaskFailedToStart", (
Issue-Key: ecs.container-depends-on-healthy-waits-for-the-health-check-to-pass|_delta#11|AssertionError|assert isinstance(code, int) and 0 <= code < _DELTA_CAP, (
Issue-Key: ecs.health-check-start-period-suppresses-unhealthy-then-expires|_poll_until#14|AssertionError|raise AssertionError(
Issue-Key: ecs.exec-interactive-stdout-round-trip|_wait_for_exec_agent#9|Timeout|time.sleep(5)
Issue-Key: ecs.execute-command-session-credentials-round-trip|_wait_for_exec_agent_ready#18|Timeout|time.sleep(interval_s)
Issue-Key: ecs.deep-ecs-3-deploy-fail-metric-alarm-rolls-back-to-rev1-quiet-twin-completes|_poll_until#25|Timeout|time.sleep(interval_s)
Issue-Key: ecs.service-cpu-utilization-metric-published|run#15|AssertionError|raise AssertionError(
Issue-Key: ecs.task-protection-enforced-during-scale-in|run#33|AssertionError|assert final_states[protected_arn]["lastStatus"] == "RUNNING", (
Issue-Key: ecs.task-state-change-ordering-enforced-rejected|run#13|AssertionError|assert code == "AccessDeniedException", (
Issue-Commit: 6ec43c73f71e6013077e783d53ce3ba5d179d34d

# ecs: 8 net-new causes — container dependsOn/health semantics are not evaluated, exec sessions never become usable, service tasks never leave PROVISIONING, no AWS/ECS metrics, rollout failure never reported, and SubmitTaskStateChange is open to non-agent callers

> _Note: this issue was identified against the code base at commit `6ec43c73f71e6013077e783d53ce3ba5d179d34d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
Eight net-new root causes under the `ecs` heading. None of these failing checks is marked ALREADY FILED or OWNED by an open issue in the replay report; several open ECS issues are adjacent and are named per cause.

Cause 1 of 8: when a container dependency condition withholds the essential container, the task's stopCode is EssentialContainerExited instead of TaskFailedToStart.
AWS: recorded behavior only for the stop classification. A dependency with condition SUCCESS whose dependency container exits non-zero means the dependent essential container is never started; the task therefore stops with stopCode 'TaskFailedToStart' (the essential container cannot have 'exited' because it never ran), and the stoppedReason names the unmet dependency. Fixed text is the stopCode enum value; the variable parts are the container and dependency names from the task definition.
Emulator: DescribeTasks reports stopCode='EssentialContainerExited' and stoppedReason='Essential container in task exited'.
Why: stopCode is the field every retry/alarm/diagnosis path branches on. 'EssentialContainerExited' sends the operator to look at an application crash that never happened, while the real cause — an unsatisfied dependsOn — is invisible.
Fix: ECS task lifecycle/dependency evaluation (containerDefinitions[].dependsOn conditions SUCCESS/COMPLETE) and the stopCode/stoppedReason it sets, in the ECS crate's task state machine.
Traces: ecs.container-depends-on-complete-admits-nonzero-exit-unlike-success.

Cause 2 of 8: a dependsOn HEALTHY dependency is not honoured and containers report no exit code at all.
AWS: recorded behavior only. With dependsOn condition HEALTHY, the dependent container starts only after the dependency's healthCheck passes, and when the dependent container exits, DescribeTasks reports its containers[].exitCode (an integer) and, for a task that stopped normally, a stoppedReason/reason describing that exit.
Emulator: `the subject (HEALTHY) app container must report its wait in seconds as its exit code, got None (reason=None, stoppedReason='Essential container in task exited')` — containers[].exitCode is absent, so neither the wait nor any in-container outcome is observable.
Why: a client cannot tell success from failure for any task whose contract is its exit code (batch work, init containers, migrations), and dependency ordering cannot be verified at all.
Fix: the ECS task data plane's container execution/exit reporting and dependsOn HEALTHY gating in the ECS crate. Adjacent open issue: 20261111T145804Z-ecs-task-data-plane-container-commands-never-execute.md (does not carry this check's replay key).
Traces: ecs.container-depends-on-healthy-waits-for-the-health-check-to-pass.

Cause 3 of 8: container healthCheck probes are never evaluated — healthStatus stays UNKNOWN forever, so startPeriod neither suppresses nor expires.
AWS: recorded behavior only. A container with a healthCheck whose command always fails reports healthStatus 'UNHEALTHY' once startPeriod has elapsed and retries are exhausted, and 'HEALTHY' when the probe passes; DescribeTasks surfaces it on both containers[].healthStatus and the task's healthStatus. During startPeriod the failing probe is suppressed (healthStatus stays UNKNOWN), and after it the status must transition — it does not stay UNKNOWN indefinitely.
Emulator: after 420s of polling both tasks still report containers[].healthStatus='UNKNOWN' and task healthStatus='UNKNOWN' while lastStatus='RUNNING', so the probe is never executed and the grace window never expires.
Why: every consumer of health — ALB target health, service steady state, deployment circuit breakers, dependsOn HEALTHY — is wired to a value that never changes, so unhealthy workloads look permanently indeterminate instead of failing.
Fix: the container health-check evaluator in the ECS task data plane (probe execution, startPeriod/interval/retries accounting) and the healthStatus it writes into task state.
Traces: ecs.health-check-start-period-suppresses-unhealthy-then-expires.

Cause 4 of 8: ECS Exec interactive sessions never become usable; both exec checks are killed at their 60s budget.
AWS: recorded behavior only. ExecuteCommand on a RUNNING task with enableExecuteCommand=true returns a session (sessionId, streamUrl, tokenValue) that opens a working data channel: the client can type into the container's PTY, read its stdout/stderr back, and read the session credentials round trip.
Emulator: both checks end with 'Check timed out after 60.0s' before any stdout/credential assertion is reached — the session never carries container output.
Why: ecs exec is the primary debugging path into a task. A session that opens but never streams leaves the client waiting indefinitely instead of failing fast with a clear error.
Fix: the ECS Exec data plane (ExecuteCommand session establishment and the stream channel that must bridge the container's PTY), in the ECS crate. Adjacent open issue: 20261104T062603Z-ecs-exec-the-executecommandagent-managed-agent-never-reaches.md (does not carry these checks' replay keys).
Traces: ecs.exec-interactive-stdout-round-trip, ecs.execute-command-session-credentials-round-trip.

Cause 5 of 8: a failing rollout with a CloudWatch-alarm-driven deployment configuration never resolves; the check exhausts its 600s budget.
AWS: recorded behavior only. With deploymentConfiguration.alarms enabled, a revision-2 rollout whose alarm goes into ALARM is failed and rolled back to the previous revision — the service's deployments report rolloutState 'FAILED' for the bad deployment and the service converges back on revision 1, while an identical service whose alarm stays OK completes to rolloutState 'COMPLETED'.
Emulator: 'Check timed out after 600.0s' — neither the rollback nor the twin's completion is ever observable.
Why: alarm-based rollback is the guardrail that limits a bad deployment's blast radius; if it never fires and the deployment never terminates, a client's deploy pipeline hangs with no verdict.
Fix: ECS deployment orchestration (deploymentConfiguration.alarms evaluation, rolloutState transitions and rollback) in the ECS crate.
Traces: ecs.deep-ecs-3-deploy-fail-metric-alarm-rolls-back-to-rev1-quiet-twin-completes.

Cause 6 of 8: ECS publishes no AWS/ECS service metrics to CloudWatch.
AWS: recorded behavior only. A service with running tasks publishes CPUUtilization in the AWS/ECS namespace dimensioned by ClusterName+ServiceName, and GetMetricStatistics/GetMetricData returns datapoints for it.
Emulator: 'AWS/ECS CPUUtilization metric did not land in CloudWatch within the poll budget' — the namespace stays empty for the service.
Why: every alarm, scaling policy and dashboard built on AWS/ECS CPUUtilization has no data, so target-tracking scaling and alerting cannot be exercised at all.
Fix: the ECS metric publication path into the cloudwatch crate (AWS/ECS namespace, ClusterName/ServiceName dimensions).
Traces: ecs.service-cpu-utilization-metric-published.

Cause 7 of 8: service-launched Fargate tasks never leave PROVISIONING, so task protection cannot be observed across a scale-in.
AWS: Smithy (ecs) UpdateTaskProtection takes cluster/tasks/protectionEnabled/expiresInMinutes and returns protectedTasks; a protected task is excluded from scale-in stop selection, so after the service's desiredCount is reduced, DescribeTasks still shows that task with lastStatus 'RUNNING' while an unprotected sibling is STOPPED. A service task reaches RUNNING within the service's normal startup window.
Emulator: the protected task is still lastStatus='PROVISIONING' (attachment status 'ATTACHING') when the scale-in is evaluated, so it never reached RUNNING and the protection semantics are unobservable.
Why: tasks stuck in PROVISIONING make every readiness, protection and scale-in behaviour untestable, and a client waiting for steady state waits forever.
Fix: the ECS task lifecycle transition PROVISIONING -> PENDING -> RUNNING for service-launched Fargate tasks (and the scale-in selection that must honour protectionEnabled), in the ECS crate.
Traces: ecs.task-protection-enforced-during-scale-in.

Cause 8 of 8: SubmitTaskStateChange is answered for a non-agent caller instead of being denied.
AWS: Smithy (ecs) SubmitTaskStateChange models AccessDeniedException with httpError 403. It is an agent-only API: a normal (non container-agent) principal is refused with error code 'AccessDeniedException' and HTTP 403 before any status validation happens, whatever status the request names.
Emulator: the call is authorized and then validated, answering error code 'InvalidParameterException' with message 'The status RUNNING is not a valid status for a task in the STOPPED status.' — a 400 from the business rule rather than the 403 refusal.
Why: a client (or a compromised caller) can drive task state transitions that only the agent may submit, and a negative test for the agent boundary passes for the wrong reason.
Fix: the authorization check in the SubmitTaskStateChange handler (agent-principal gate, ordered before status validation) in the ECS crate. Adjacent open issue: 20261111T233326Z-ecs-submittaskstatechange-is-reachable-from-a-non-agent.md (does not carry this check's replay key).
Traces: ecs.task-state-change-ordering-enforced-rejected.

## Evidence
Cause 1: AssertionError: condition SUCCESS withholding app must surface as the task's own stopCode='TaskFailedToStart' ...; got stopCode='EssentialContainerExited' stoppedReason='Essential container in task exited'.
Cause 2: AssertionError: the subject (HEALTHY) app container must report its wait in seconds as its exit code, got None (reason=None, stoppedReason='Essential container in task exited').
Cause 3: AssertionError: timed out after 420.0s waiting for both tasks eventually report healthStatus UNHEALTHY ...; last observed containers[0].healthStatus='UNKNOWN', task healthStatus='UNKNOWN', lastStatus='RUNNING'.
Cause 4: Error: Check timed out after 60.0s (ecs.exec-interactive-stdout-round-trip, ecs.execute-command-session-credentials-round-trip).
Cause 5: Error: Check timed out after 600.0s (no rolloutState FAILED, no rollback to revision 1).
Cause 6: AssertionError: AWS/ECS CPUUtilization metric did not land in CloudWatch within the poll budget.
Cause 7: AssertionError: the protected task must be excluded from scale-in stop selection, got {... 'lastStatus': 'PROVISIONING', attachments[0].status 'ATTACHING' ...} — expected lastStatus 'RUNNING'.
Cause 8: AssertionError: expected SubmitTaskStateChange to be denied to a non-agent caller with AccessDeniedException, got code='InvalidParameterException' message='The status RUNNING is not a valid status for a task in the STOPPED status.'

## Steps to Reproduce
Cause 1: register a task definition with a non-essential dependency container that exits non-zero and an essential container with dependsOn condition SUCCESS; RunTask and read DescribeTasks stopCode/stoppedReason.
Cause 2: register a definition whose app container dependsOn another with condition HEALTHY (the dependency carries a healthCheck); RunTask, wait for STOPPED and read containers[].exitCode.
Cause 3: run two tasks with a container healthCheck that always fails, one with healthCheck.startPeriod=120; poll DescribeTasks healthStatus for >120s.
Cause 4: RunTask with enableExecuteCommand=true, then ecs ExecuteCommand --interactive and attempt a stdout round trip / read the session credentials.
Cause 5: create a service on revision 1, set deploymentConfiguration.alarms to a CloudWatch alarm, update to a failing revision 2 and drive the alarm to ALARM; poll DescribeServices deployments[].rolloutState.
Cause 6: create a service with running tasks, then cloudwatch GetMetricStatistics(Namespace='AWS/ECS', MetricName='CPUUtilization', Dimensions=[ClusterName, ServiceName]).
Cause 7: create a Fargate service with desiredCount=2, UpdateTaskProtection(protectionEnabled=true) on one task, reduce desiredCount to 1 and poll DescribeTasks for both tasks' lastStatus.
Cause 8: as an ordinary (non-agent) principal call ecs SubmitTaskStateChange(cluster, task, status='RUNNING') for a STOPPED task and inspect the error code and HTTP status.
