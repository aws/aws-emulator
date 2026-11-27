Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.health-check-start-period-suppresses-unhealthy-then-expires ecs.e2e-service-deployment-rollback-on-failed-taskdef ecs.task-state-change-server-enforced
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.health-check-start-period-suppresses-unhealthy-then-expires|_statuses#14|AssertionError|assert task["lastStatus"] != "STOPPED", (
Issue-Key: ecs.e2e-service-deployment-rollback-on-failed-taskdef|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: ecs.task-state-change-server-enforced|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Commit: 0e9afbb76f97c880d990543e0ccf2d5bfecf65a1

# ecs: 3 net-new causes — an essential container running `sleep 900` with a failing healthCheck is stopped as EssentialContainerExited, circuit-breaker rollback never converges, and the SubmitTaskStateChange regression guard is unreachable

> _Note: this issue was identified against the code base at commit `0e9afbb76f97c880d990543e0ccf2d5bfecf65a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Three root causes, grounded in 3 failing checks the suite report lists under `### ecs`, none of which is marked "ALREADY FILED" or "OWNED by" any open issue.

Cause 1 of 3: a Fargate task whose single essential container runs a long-lived command and carries an always-failing container health check is STOPPED by the emulator as if the container had exited, instead of staying RUNNING and reporting healthStatus UNHEALTHY once the startPeriod grace window expires.

  AWS: Smithy (com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113) models `HealthCheck` {command (required), interval 5-300s default 30, timeout 2-60s default 5, retries 1-10 default 3, startPeriod 0-300s, off by default} and documents it: "The optional grace period to provide containers time to bootstrap before failed health checks count towards the maximum number of retries", and for `healthStatus`: UNHEALTHY — one or more essential containers failed their health check. A standalone task created by `RunTask` (not a service task) whose container health check fails is REPORTED unhealthy, not killed: `DescribeTasks` keeps `lastStatus: RUNNING` and the container record's `healthStatus` flips to `UNHEALTHY` only after the startPeriod has elapsed, while an otherwise identical task registered WITHOUT a startPeriod turns UNHEALTHY after roughly retries*interval. The container's own process (`sh -c "sleep 900"`) is still alive, so `stopCode`/`stoppedReason` must be absent. The timing itself is recorded behavior only; the model states the fields and the grace-period rule, not the clock.

  Emulator: within the observation window both probe tasks are already gone. `DescribeTasks` reports `lastStatus: "STOPPED"` with `stopCode='EssentialContainerExited'` and `stoppedReason='Essential container in task exited'` for a container whose command sleeps for 900 seconds — so either the emulator never actually keeps the container's command running, or it treats a failing `healthCheck` as the essential container exiting. Either way the task is terminated and no healthStatus trajectory exists to read.

  Why: a client cannot distinguish "unhealthy but serving" from "dead". Health-check configuration becomes unobservable: startPeriod suppression, the UNHEALTHY verdict after it, and the AWS rule that a standalone task with an unhealthy container is reported rather than replaced all collapse into an immediate task stop with a stop reason that is factually wrong (nothing exited). Anything that keys off `stopCode` (retry logic, alarms, dashboards) is driven by a fabricated terminal event.

  Fix: the task/container lifecycle in the ECS crate — the task state machine and container health evaluation behind `RunTask`/`DescribeTasks` (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/run_task.rs, .../src/ops/describe_tasks.rs and the task records in .../src/state.rs). The container's command must keep the task RUNNING, and a failing healthCheck must set the container's and task's `healthStatus`, honouring `startPeriod`, without producing a stop.

  Traces: ecs.health-check-start-period-suppresses-unhealthy-then-expires.

Cause 2 of 3: with `deploymentCircuitBreaker {enable: true, rollback: true}`, updating a service to a revision whose image can never be pulled never produces a FAILED deployment and never rolls back; the whole rollout lifecycle produces no observable transition and the check is cut by the harness at 60s.

  AWS: Smithy models `Deployment.rolloutState` as the enum DeploymentRolloutState {IN_PROGRESS, COMPLETED, FAILED} with `rolloutStateReason`, and documents the rule: "When a service deployment is started, it begins in an IN_PROGRESS state. When the service reaches a steady state, the deployment transitions to a COMPLETED state. If the service fails to reach a steady state and circuit breaker is turned on, the deployment transitions to a FAILED state. A deployment in FAILED state doesn't launch any new tasks." `DeploymentCircuitBreaker.rollback` "Determines whether to configure Amazon ECS to roll back the service if a service deployment fails". So: the initial deployment of a pullable revision settles with one RUNNING task on that revision; `UpdateService(taskDefinition=<unpullable revision>)` makes that revision PRIMARY; the circuit breaker then marks that deployment `rolloutState: FAILED` with a `rolloutStateReason` naming the circuit breaker; the running tasks settle back on the prior stable revision with no task of the rejected revision left; and `DescribeServices` shows a single steady deployment on the prior revision with `runningCount == desiredCount`. `ListServiceDeployments`/`DescribeServiceDeployments` must surface a record for the same service (`serviceDeployments[].serviceArn` == the service ARN). The reason text is AWS's own fixed text for this class and must contain "circuit breaker"; the deployment ids, ARNs and revision numbers in it are per-run values, not part of the requirement.

  Emulator: nothing in that sequence is observed. The body polls `ListTasks`/`DescribeTasks` for one RUNNING task on the good revision, then `DescribeServices` for the bad deployment's `rolloutState == "FAILED"`, and is cut before any of it: `Error: Check timed out after 60.0s`. No error and no success is produced — the service simply never advances through a rollout, so neither the FAILED verdict nor the rollback can be graded.

  Why: a client using the circuit breaker as its safety net gets silence. A bad revision is neither rejected nor rolled back and `rolloutState` never becomes FAILED, so automation that waits for COMPLETED/FAILED hangs forever and a deployment that AWS would have reverted is left indistinguishable from one still in progress.

  Fix: the service scheduler / deployment bookkeeping in the ECS crate — `UpdateService`'s deployment transitions and the circuit-breaker evaluation (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/update_service.rs, .../src/ops/describe_services.rs and the service/deployment records in .../src/state.rs), plus the ListServiceDeployments/DescribeServiceDeployments views over the same records.

  Traces: ecs.e2e-service-deployment-rollback-on-failed-taskdef.

Cause 3 of 3: the agent-facing `SubmitTaskStateChange` regression guard cannot be exercised at all, because the EC2 instance the check launches never becomes a registered container instance, so `StartTask` never has a host.

  AWS: Smithy models `SubmitTaskStateChange` (cluster, task, status, reason, containers, attachments, managedAgents, pullStartedAt, pullStoppedAt, executionStoppedAt) with errors AccessDeniedException, ClientException, InvalidParameterException, ServerException. Recorded behavior: `SubmitTaskStateChange` against an unknown cluster is refused HTTP 400 with `Error.Code = "ClientException"` and the fixed message `Cluster not found.` (no request values in it) — this leg the emulator already satisfies. Past it, an EC2 instance launched from the ECS-optimized AMI with an ECS instance profile and `ECS_CLUSTER=<cluster>` user data must register, so `StartTask --container-instances <ci>` places a short-lived task that reaches STOPPED naturally, and a stale `SubmitTaskStateChange(status=RUNNING)` for that task must NOT regress `DescribeTasks`'s `lastStatus` away from STOPPED.

  Emulator: the check passes the ClientException leg and is then cut inside the container-instance registration poll: `Error: Check timed out after 60.0s`. `ListContainerInstances`/`DescribeContainerInstances` never return an instance carrying the launched `ec2InstanceId`, so `StartTask` and the stale-state-change assertion are never reached.

  Why: the server-side ordering guarantee that protects task state from a stale agent report is unverifiable and, if absent, would let a late RUNNING report resurrect a STOPPED task in a client's view.

  Fix: the same ECS-on-EC2 registration path as the related open issue below, plus `SubmitTaskStateChange`'s ordering guard (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/register_container_instance.rs, .../src/ops/submit_task_state_change.rs, .../src/state.rs).

  Related open issue on the same registration gap which does NOT own this check (the report marks ecs.task-state-change-server-enforced neither ALREADY FILED nor OWNED): 20261120T023605Z-ecs-an-ec2-instance-launched-with-an-ecs.md. Closing that issue without replaying ecs.task-state-change-server-enforced leaves this check unrun.

  Traces: ecs.task-state-change-server-enforced.

## Evidence
Cause 1 — expected (recorded AWS): `RegisterTaskDefinition` with one FARGATE container {name: app, image: busybox, essential: true, command: ["sh","-c","sleep 900"], healthCheck: {command: ["CMD-SHELL","exit 1"], interval: 5, timeout: 2, retries: 2, startPeriod: 120}} and a twin without startPeriod; `RunTask` both; `DescribeTasks` keeps BOTH at `lastStatus: "RUNNING"` for the whole observation, the no-grace twin reports `healthStatus: "UNHEALTHY"` within ~retries*interval, the startPeriod twin is not UNHEALTHY before its window ends and is UNHEALTHY after it, and the final read shows `containers[name=app].healthStatus == "UNHEALTHY"` with `lastStatus == "RUNNING"` and no stopCode/stoppedReason.

Cause 1 — actual (emulator):

  File "check_ecs_health_check_start_period_suppresses_unhealthy_then_expires.py", line 180, in _statuses
    assert task["lastStatus"] != "STOPPED"
  AssertionError: task arn:aws:ecs:us-west-2:000000000000:task/<cluster>/<task-id> stopped while its health status was under observation, so the timing measurement is void: stopCode='EssentialContainerExited' stoppedReason='Essential container in task exited'

(The cluster name, account and task id are per-run generated values. The requirement is that a task whose essential container is still sleeping must NOT be STOPPED, and must not carry `stopCode: EssentialContainerExited` / `stoppedReason: Essential container in task exited`.)

Cause 2 — expected (recorded AWS): after `CreateService(... taskDefinition=<good>, desiredCount=1, launchType=FARGATE, deploymentConfiguration={deploymentCircuitBreaker:{enable:true,rollback:true}})`, `ListTasks(serviceName)` + `DescribeTasks` show exactly one RUNNING task on `<good>`; after `UpdateService(taskDefinition=<unpullable>)`, `DescribeServices(...).services[0].deployments` contains an entry for `<unpullable>` with `rolloutState: "FAILED"` and a `rolloutStateReason` naming the circuit breaker; the running tasks settle back on `<good>` with no task on `<unpullable>`; the PRIMARY deployment references `<good>` with `runningCount == desiredCount == 1`; `ListServiceDeployments` returns at least one summary and `DescribeServiceDeployments` reports `serviceArn` equal to the service's ARN.

Cause 2 — actual (emulator): `Error: Check timed out after 60.0s` — cut inside the first polling loop, with no rollout state ever changing. Neither a 200 carrying a transition nor a modeled error is produced.

Cause 3 — expected (recorded AWS): `SubmitTaskStateChange --cluster <no-such-cluster> --task <32 chars> --status RUNNING` -> HTTP 400 `("ClientException", "Cluster not found.")` (satisfied today); then an ECS-optimized EC2 instance with `ECS_CLUSTER=<cluster>` user data registers, `ListContainerInstances` returns its ARN, `StartTask --container-instances <ci> --task-definition <td>` returns `tasks[0]` with no `failures`, the short-lived task reaches `lastStatus: "STOPPED"`, and a following `SubmitTaskStateChange(status=RUNNING)` leaves `DescribeTasks`'s `lastStatus` at STOPPED.

Cause 3 — actual (emulator): `Error: Check timed out after 60.0s` — the ClientException leg passes, then `ListContainerInstances`/`DescribeContainerInstances` never show the launched instance, so `StartTask` and the regression assertion are never reached.

## Steps to Reproduce
Cause 1:
1. `ecs create-cluster --cluster-name <c>`.
2. `ecs register-task-definition --family <f-grace> --requires-compatibilities FARGATE --network-mode awsvpc --cpu 256 --memory 512 --container-definitions '[{"name":"app","image":"public.ecr.aws/docker/library/busybox:1.36","essential":true,"command":["sh","-c","sleep 900"],"healthCheck":{"command":["CMD-SHELL","exit 1"],"interval":5,"timeout":2,"retries":2,"startPeriod":120}}]'` and the same without `startPeriod` as `<f-nograce>`.
3. `ecs run-task --cluster <c> --launch-type FARGATE --task-definition <each> --network-configuration awsvpcConfiguration={subnets=[<s>],securityGroups=[<sg>],assignPublicIp=ENABLED}`.
4. Poll `ecs describe-tasks --cluster <c> --tasks <both>`: on the emulator the tasks go `lastStatus: STOPPED` with `stopCode: EssentialContainerExited`; on AWS both stay RUNNING, the no-grace one reports `healthStatus: UNHEALTHY` quickly, the grace one only after its 120s window, and neither is stopped.

Cause 2:
1. `ecs create-cluster --cluster-name <c>`; register a pullable FARGATE revision `<good>` (`sleep 3600`).
2. `ecs create-service --cluster <c> --service-name <s> --task-definition <good> --desired-count 1 --launch-type FARGATE --network-configuration ... --deployment-configuration deploymentCircuitBreaker={enable=true,rollback=true}`.
3. Poll `ecs list-tasks --cluster <c> --service-name <s> --desired-status RUNNING` + `describe-tasks` until one task runs `<good>`.
4. Register `<bad>` in the same family with an image that cannot be pulled; `ecs update-service --cluster <c> --service <s> --task-definition <bad>`.
5. Poll `ecs describe-services --cluster <c> --services <s>`: the `<bad>` deployment must reach `rolloutState: FAILED` with a `rolloutStateReason` containing "circuit breaker"; on the emulator no deployment state ever changes.
6. Then verify the rollback: running tasks back on `<good>` only, PRIMARY deployment on `<good>` with `runningCount == desiredCount == 1`, and `ecs list-service-deployments --cluster <c> --service <s>` + `ecs describe-service-deployments` returning a record whose `serviceArn` is the service's ARN.

Cause 3:
1. `ecs create-cluster --cluster-name <c>`.
2. `ecs submit-task-state-change --cluster no-such-cluster-<x> --task 00000000000000000000000000000000 --status RUNNING` -> expect 400 `ClientException` / `Cluster not found.` (already correct).
3. Launch an EC2 instance from `/aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id` with an `AmazonEC2ContainerServiceforEC2Role` instance profile and user data `echo ECS_CLUSTER=<c> >> /etc/ecs/ecs.config`; poll `ecs list-container-instances --cluster <c>` — on the emulator it stays empty.
4. With the instance registered: `ecs start-task --cluster <c> --container-instances <ci> --task-definition <td with command sh -c 'sleep 2'>`, wait for `lastStatus: STOPPED`, then `ecs submit-task-state-change --cluster <c> --task <t> --status RUNNING` and confirm `ecs describe-tasks` still reports STOPPED.
