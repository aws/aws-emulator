Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.container-depends-on-healthy-waits-for-the-health-check-to-pass ecs.container-health-check-transitions-healthy-and-unhealthy
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.container-depends-on-healthy-waits-for-the-health-check-to-pass|run#14|AssertionError|assert subject_delta >= _READY_DELAY_S, (
Issue-Key: ecs.container-health-check-transitions-healthy-and-unhealthy|run#24|AssertionError|assert unhealthy["lastStatus"] == "RUNNING", (
Issue-Commit: 244eae870d957920c579d40eef12d136da6f5bab

# ecs: 2 net-new causes — a dependsOn condition of HEALTHY is honoured as if it were START, and a standalone task with a failing container health check is stopped as EssentialContainerExited instead of reported UNHEALTHY

> _Note: this issue was identified against the code base at commit `244eae870d957920c579d40eef12d136da6f5bab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Two root causes, grounded in 2 failing checks the suite report lists under `### ecs`, neither marked "ALREADY FILED" nor "OWNED by" an open issue.

Cause 1 of 2: a container dependency whose condition is HEALTHY does not hold the dependent container until the depended-on container's healthCheck passes — the dependent starts as soon as the dependency has STARTED, i.e. HEALTHY is implemented as START.

  AWS: Smithy (com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113) models ContainerDependency {containerName (required), condition (required, enum ContainerCondition [START, COMPLETE, SUCCESS, HEALTHY])} and documents each condition: "START - ... validates that a dependent container is started before permitting other containers to start" versus "HEALTHY - This condition validates that the dependent container passes its Docker health check before permitting other containers to start. This requires that the dependent container has health checks configured. This condition is confirmed only at task startup." HealthCheck is modelled with command (required), interval (5-300, default 30), timeout (2-60, default 5), retries (1-10, default 3) and startPeriod (0-300, off by default). So in a task where container `db` becomes healthy only after a delay (its healthCheck command only succeeds once `db` has created its readiness marker) and container `app` declares dependsOn [{containerName: db, condition: HEALTHY}], `app`'s start time must lag `db`'s start time by at least that readiness delay, whereas an otherwise identical task definition using condition START shows essentially no lag. The exact delay is the scenario's own constant, not an AWS constant; the requirement is the ORDERING guarantee that HEALTHY waits for the health result and START does not.

  Emulator: the two task definitions are indistinguishable. In the HEALTHY task, `app` started only 8s after `db` — far short of the readiness delay (15s) that `db`'s health check cannot pass before — while the identically built START control measured 0s. The emulator gates on "dependency started", never on the dependency's health result, so the two conditions produce the same schedule.

  Why: dependency ordering is the documented way to keep an application container from talking to a database/sidecar/proxy before it is ready. A client that relies on HEALTHY gets its container launched against a not-yet-ready dependency, producing connection failures and crash loops at startup that cannot happen on AWS — and the difference between START and HEALTHY becomes unobservable, so the stricter contract cannot be tested against the emulator at all.

  Fix: the task-startup orchestration in the ECS crate — the container start ordering behind RunTask/StartTask must evaluate the ContainerDependency condition per enum value, and for HEALTHY block the dependent container until the depended-on container's healthCheck has reported success (as reflected in the container record's healthStatus), rather than on the dependency's start.

  Traces: ecs.container-depends-on-healthy-waits-for-the-health-check-to-pass.

Cause 2 of 2: a standalone (non-service) task whose essential container keeps running but fails its container health check is TERMINATED by the emulator with a fabricated exit reason, instead of staying RUNNING with healthStatus UNHEALTHY.

  AWS: Smithy models HealthStatus as the enum [HEALTHY, UNHEALTHY, UNKNOWN] and documents it on HealthCheck: "HEALTHY - The container health check has passed successfully", "UNHEALTHY - The container health check has failed", "UNKNOWN - The container health check is being evaluated, there's no container health check defined, or Amazon ECS doesn't have the health status of the container", and for the task, "UNHEALTHY - One or more essential containers have failed their health check". The health status is a REPORTED attribute, surfaced through DescribeTasks for both the container and the task: ECS does not replace or stop a standalone task because it is unhealthy (that behaviour belongs to a SERVICE's scheduler, which replaces unhealthy service tasks). So a RunTask-created task whose container command is still alive must keep `lastStatus: RUNNING`, must carry no stopCode/stoppedReason, and must flip containers[].healthStatus between HEALTHY and UNHEALTHY as the health-check command's exit status changes.

  Emulator: DescribeTasks reports the task `lastStatus: "STOPPED"` with `stoppedReason: "Essential container in task exited"` while the container's own command is still supposed to be running — so the emulator either does not keep the container's process alive or converts a failing healthCheck into an essential-container exit. Either way the UNHEALTHY-then-HEALTHY trajectory is unobservable and the reported stop reason is factually wrong: nothing exited.

  Why: a client cannot distinguish "unhealthy but still serving" from "dead". Health-check configuration becomes untestable, and anything keyed off stopCode/stoppedReason (retry logic, alarms, dashboards, batch drivers that treat EssentialContainerExited as a real exit) is driven by a terminal event AWS never produced. Code that deliberately runs an unhealthy-but-alive task to observe degradation sees it vanish.

  Fix: the task/container lifecycle in the ECS crate — the container health evaluation and the task state machine behind RunTask/DescribeTasks must set containers[].healthStatus (and the task's healthStatus) from the health-check result, and must never derive a stop or a stoppedReason of "Essential container in task exited" from a failing health check for a standalone task.

  Related open issue that does NOT own this check (the report marks ecs.container-health-check-transitions-healthy-and-unhealthy neither ALREADY FILED nor OWNED): 20261127T093558Z-ecs-3-net-new-causes-an-essential-container.md, whose Cause 1 covers the startPeriod grace window on the same stop-vs-report confusion. Closing that issue without replaying this check leaves the HEALTHY/UNHEALTHY transition requirement unrun.

  Traces: ecs.container-health-check-transitions-healthy-and-unhealthy.

## Evidence
Cause 1 — expected (AWS): two FARGATE/awsvpc task definitions built identically except for the dependency condition. Both contain `db` (a long-lived command that sleeps the readiness delay, then creates a readiness marker on a shared volume, with healthCheck command ["CMD-SHELL", "test -f /shared/ready"], interval 5) and `app` (which records its own start time) with dependsOn [{containerName: db, condition: <HEALTHY|START>}]. RunTask both: the HEALTHY task's measured app-minus-db start delta is >= the readiness delay, the START control's delta is ~0.

Cause 1 — actual (emulator):

  File "check_ecs_container_depends_on_healthy_waits_for_the_health_check_to_pass.py", line 60, in run
    assert subject_delta >= _READY_DELAY_S
  AssertionError: condition HEALTHY must hold app until db's healthCheck passes, which cannot happen before db has slept 15s and created /shared/ready; app started only 8s after db (the identically-built START control measured 0s), which is what a target that treats HEALTHY as START looks like

(15s is the scenario's own readiness delay and 8s/0s are this run's measurements; the requirement is that the HEALTHY delta is at least the readiness delay while the START delta is not.)

Cause 2 — expected (AWS): RunTask on a FARGATE task definition whose essential container runs a long-lived command and carries a healthCheck whose command can be flipped between failing and succeeding; DescribeTasks keeps `lastStatus: "RUNNING"` throughout, with no stopCode and no stoppedReason, and `containers[0].healthStatus` observed as UNHEALTHY while the probe fails and HEALTHY once it succeeds.

Cause 2 — actual (emulator):

  File "check_ecs_container_health_check_transitions_healthy_and_unhealthy.py", line 215, in run
    assert unhealthy["lastStatus"] == "RUNNING"
  AssertionError: a standalone (non-service) task must NOT be killed for being unhealthy, got lastStatus='STOPPED' stoppedReason='Essential container in task exited'

(The fixed text at issue is the emulator's stoppedReason "Essential container in task exited"; AWS emits no stop at all here, so lastStatus must remain "RUNNING" and both stopCode and stoppedReason must be absent. Task ARNs, cluster names and account ids in the record are per-run values.)

## Steps to Reproduce
Cause 1:
1. `ecs create-cluster --cluster-name <c>`.
2. Register two FARGATE/awsvpc revisions that differ only in the dependency condition, each with a shared volume and containers:
   - `db`: command that sleeps 15s, then `touch /shared/ready`, then stays alive; `healthCheck: {"command":["CMD-SHELL","test -f /shared/ready"],"interval":5,"timeout":2,"retries":3}`.
   - `app`: records its start time; `dependsOn: [{"containerName":"db","condition":"HEALTHY"}]` in the subject revision and `"condition":"START"` in the control revision.
3. `ecs run-task --cluster <c> --launch-type FARGATE --task-definition <each> --network-configuration awsvpcConfiguration={subnets=[<s>],securityGroups=[<sg>],assignPublicIp=ENABLED}`.
4. Let both tasks finish and compare each task's app-start minus db-start delta: on AWS the HEALTHY revision's delta is >= 15s and the START revision's is ~0; on the emulator both are ~0 (8s and 0s observed).

Cause 2:
1. `ecs create-cluster --cluster-name <c>`.
2. Register a FARGATE/awsvpc revision with one essential container running a long-lived command and a healthCheck whose probe depends on a file the container controls (e.g. `["CMD-SHELL","test -f /tmp/ok"]`, interval 5, timeout 2, retries 2).
3. `ecs run-task --cluster <c> --launch-type FARGATE --task-definition <td> --network-configuration ...`.
4. Poll `ecs describe-tasks --cluster <c> --tasks <t>` while the probe fails: on AWS `lastStatus` stays `RUNNING` with `containers[0].healthStatus == "UNHEALTHY"` and no stopCode/stoppedReason; on the emulator the task becomes `lastStatus: STOPPED` with `stoppedReason: "Essential container in task exited"`.
5. Make the probe succeed and confirm `healthStatus` returns to `HEALTHY` with the task still RUNNING.
