Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__logs.obs-ecs-2-fargate-service-metrics-dimensioned-per-service-not-per-cluster
Issue-Key: cloudwatch__ecs__logs.obs-ecs-2-fargate-service-metrics-dimensioned-per-service-not-per-cluster|_poll_until#7|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 76a27d9bdd16e996b43ee8be0e1161c3fa717030

# A Fargate service never reports runningCount >= 1, so AWS/ECS per-service CPU/Memory series are never produced

> _Note: this issue was identified against the code base at commit `76a27d9bdd16e996b43ee8be0e1161c3fa717030`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
One net-new root cause, grounded in the failing behavioral check under the suite heading `cloudwatch__ecs__logs`.

Cause 1 of 1: a Fargate service created with desiredCount 1 never reports a running task, so the check cannot get past its first gate and the AWS/ECS per-service metric contract is never reached.
AWS: Smithy: ecs#Service.runningCount is "The number of tasks in the cluster that are in the RUNNING state"; after CreateService with desiredCount 1 on a task definition with a long-lived command, DescribeServices reports runningCount 1 == desiredCount 1, and AWS/ECS then publishes CPUUtilization and MemoryUtilization (Unit Percent) dimensioned exactly ClusterName+ServiceName for that service. Recorded behavior only for the convergence and metric latency, which the check polls with a 600 s budget.
Emulator: the check exhausts its 600 s budget on its first poll — `service <name> runningCount >= 1` — so the service's task never becomes RUNNING and no per-service AWS/ECS series, and no alarm on that series, can ever exist.
Why: a client cannot bring up a Fargate service at all in this path: deployment waiters never return, per-service CPU/Memory dashboards and alarms stay empty/INSUFFICIENT_DATA, and the distinction AWS makes (per-service series exist, ClusterName-only series do not for a Fargate-only cluster) cannot be observed.
Fix: the ECS service scheduler must create tasks for a service and count the RUNNING ones into Service.runningCount, and the AWS/ECS metric publisher must emit CPUUtilization/MemoryUtilization in Percent dimensioned ClusterName+ServiceName for each running service (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src and its cloudwatch metric emission). Same family as the task-lifecycle causes filed for headings cloudwatch__ecr__ecs and cloudwatch__ecs__elbv2__logs; filed separately because the grading heading differs.
Traces: cloudwatch__ecs__logs.obs-ecs-2-fargate-service-metrics-dimensioned-per-service-not-per-cluster.

## Evidence
Error: Check timed out after 600.0s. The check's first blocking step is _poll_until(describe_services(...)['services'][0], runningCount >= 1, timeout_s=600, interval_s=10, description="service <name> runningCount >= 1") on a cluster created with containerInsights disabled and one Fargate service (desiredCount 1, image public.ecr.aws/docker/library/busybox:1.36).

Expected (AWS): DescribeServices reports runningCount 1 / desiredCount 1 within minutes, then AWS/ECS CPUUtilization and MemoryUtilization datapoints appear in Percent with dimensions ClusterName+ServiceName.
Actual (emulator): runningCount never reaches 1 within 600 s, so the metric legs are never reached.

## Steps to Reproduce
1. Create a VPC/subnets, an ECS cluster with settings [{name: containerInsights, value: disabled}], and RegisterTaskDefinition (FARGATE, awsvpc, busybox:1.36, long-lived command).
2. CreateService with desiredCount 1 and poll DescribeServices for runningCount >= 1: it never arrives within 600 s.
3. (Blocked by step 2) Poll GetMetricStatistics in AWS/ECS for CPUUtilization/MemoryUtilization with dimensions ClusterName+ServiceName: no series exists.
