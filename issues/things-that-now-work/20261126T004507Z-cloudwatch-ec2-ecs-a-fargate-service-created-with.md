Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__ecs.metrics-ec2-ecs-update-service-negative-desired-count-rejected
Issue-Key: cloudwatch__ec2__ecs.metrics-ec2-ecs-update-service-negative-desired-count-rejected|_poll_until#14|AssertionError|raise AssertionError(f"{description} did not happen within {timeout_s}s; last read {last!r}")
Issue-Commit: 9f7bf23473ff024f6e39d27cef35bcce93712a4b

# cloudwatch__ec2__ecs: a Fargate service created with desiredCount=1 never reaches runningCount=1, so the UpdateService negative-desiredCount refusal and its RunningTaskCount metric are unreachable

> _Note: this issue was identified against the code base at commit `9f7bf23473ff024f6e39d27cef35bcce93712a4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Cause 1 of 1: an ECS Fargate service created with desiredCount=1 on an awsvpc task definition never places a task that reaches RUNNING. DescribeServices keeps reporting runningCount=0 for the whole budget, so the check cannot even reach the behavior it exists to verify.

AWS: recorded behavior only for the placement timeline (the ECS Smithy model types Service.runningCount as an integer and states no timing). What AWS guarantees and the recording shows: CreateService with launchType=FARGATE, desiredCount=1, networkMode=awsvpc and a public subnet places one task, the task reaches RUNNING, and DescribeServices reports runningCount=1 (converging on desiredCount) well within the check's 420 s budget. The behaviors gated behind that state are also AWS-modeled: Smithy UpdateService takes `desiredCount: BoxedInteger` and declares `InvalidParameterException` (client error, HTTP 400); AWS refuses desiredCount=-1 with code `InvalidParameterException` and message `Task count can not be lesser than 0` (that whole sentence is fixed text — it carries no value from the request), leaves the service at desiredCount=1/runningCount=1, and then honors UpdateService(desiredCount=2) so that AWS/ECS/ContainerInsights `RunningTaskCount` (Unit `Count`, dimensions ClusterName/ServiceName) reaches 2 and never reads outside 0..2.

Emulator: the service is created and accepted, but runningCount stays 0: `AssertionError: service sim2real-xsvc-ci-1-<id> reaching runningCount=1 did not happen within 420.0s; last read 0`. The service name and id are per-run values. Because the service never converges, the UpdateService(-1) refusal (status/code/message), the no-op-on-refusal assertion, the UpdateService(2) positive control and the RunningTaskCount series are all never exercised.

Why: a client that creates a Fargate service and waits for it to become steady (the services-stable waiter, or any poll on runningCount==desiredCount) waits forever against the emulator. Everything downstream of a steady service — deployments, scaling, Container Insights task-count metrics, load-balancer registration — is unreachable, and an automation that gates on steady state hangs rather than failing.

Fix: the ECS service scheduler/state component that must place tasks for a Fargate service and advance Service.runningCount toward desiredCount (and emit the corresponding ECS/ContainerInsights RunningTaskCount series). Note the existing open issue 20261125T220211Z-cloudwatch-ec2-ecs-ecs-fargate-service-tasks-never-reach-runningcount-desiredcount.md, which describes the same shape of failure for a different check and does not carry this check's replay key; this check is not marked owned by it, so it is filed here with that issue named.

Traces: cloudwatch__ec2__ecs.metrics-ec2-ecs-update-service-negative-desired-count-rejected.

## Evidence
Check: cloudwatch__ec2__ecs.metrics-ec2-ecs-update-service-negative-desired-count-rejected

Expected (AWS):
  - DescribeServices on the just-created Fargate service reaches runningCount=1 (converging on desiredCount=1).
  - UpdateService(desiredCount=-1) -> HTTP 400, code `InvalidParameterException`, message `Task count can not be lesser than 0` (fixed text, no request-derived parts), and the service stays at desiredCount=1 / runningCount=1.
  - UpdateService(desiredCount=2) takes effect; AWS/ECS/ContainerInsights RunningTaskCount (Unit `Count`) reaches 2 and only ever reads 0, 1 or 2.

Actual (emulator):
  File "check_cloudwatch__ec2__ecs_metrics_ec2_ecs_update_service_negative_desired_count_rejected.py", line 49, in run
    _poll_until(lambda: _svc(ecs, cluster, svc)["runningCount"], lambda n: n == 1, timeout_s=420.0, ...)
  AssertionError: service sim2real-xsvc-ci-1-9599150e75e8 reaching runningCount=1 did not happen within 420.0s; last read 0

The service name/suffix is a per-run value. runningCount never leaves 0, so none of the UpdateService or metric assertions are ever evaluated.

Smithy (ecs): UpdateService input `desiredCount: BoxedInteger`; declared errors include `InvalidParameterException` (client, httpError 400). Output `service: Service` carries runningCount/desiredCount.

## Steps to Reproduce
1. Create a VPC with a public subnet and a security group (public network with an internet gateway and default route).
2. CreateCluster with setting containerInsights=enabled.
3. RegisterTaskDefinition: networkMode=awsvpc, requiresCompatibilities=[FARGATE], cpu=256, memory=512, one container running `sh -c "sleep 3600"`.
4. CreateService with launchType=FARGATE, desiredCount=1, awsvpc networkConfiguration on that subnet/SG.
5. Poll DescribeServices for runningCount==1. It stays 0 past 420 s; AWS reaches 1.
6. (Unreachable today) UpdateService(desiredCount=-1) and assert 400 InvalidParameterException / `Task count can not be lesser than 0` with no state change; then UpdateService(desiredCount=2) and assert ECS/ContainerInsights RunningTaskCount reaches 2 with Unit=Count.
