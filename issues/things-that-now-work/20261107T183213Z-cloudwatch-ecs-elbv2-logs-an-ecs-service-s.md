Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.alb-security-group-mismatch-blocks-health-checks
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-security-group-mismatch-blocks-health-checks|run#20|AssertionError|assert last_states, "target was never registered in the target group"
Issue-Commit: e0dc03155a79e3340fd5a6f413ff0e6f3f485ded

# cloudwatch__ecs__elbv2__logs: an ECS service's task is never registered into its target group, so the ALB security-group-mismatch negative check can never observe an unhealthy target

> _Note: this issue was identified against the code base at commit `e0dc03155a79e3340fd5a6f413ff0e6f3f485ded`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
One net-new root cause (1 of 1) behind the one surviving unmarked cloudwatch__ecs__elbv2__logs failure.

Related open filings that do NOT carry this replay key: 20261104T062603Z-cloudwatch-ecs-elbv2-logs-task-attachments-expose-no.md and 20261105T143800Z-cloudwatch-ecs-elbv2-logs-3-net-new-causes.md own the other 15 failures in this service; this check is listed unmarked and survives.

Cause 1 of 1: a load-balanced ECS service's RUNNING task never appears in elbv2 target health, so a security-group mismatch cannot be distinguished from a service that registered nothing.
AWS: recorded behavior, with the representation modeled. Smithy: elbv2 DescribeTargetHealth(TargetGroupArn required, Targets, Include) -> DescribeTargetHealthOutput{TargetHealthDescriptions}; TargetHealth.State is TargetHealthStateEnum [initial, healthy, unhealthy, unhealthy.draining, unused, draining, unavailable] and TargetHealth.Reason is TargetHealthReasonEnum, where the model documents 'Target.Timeout — The health check requests timed out' and 'Target.FailedHealthChecks — The load balancer received an error while establishing a connection to the target' for state 'unhealthy'. Recorded AWS behavior: ECS registers the service's task into the target group as soon as it is RUNNING; when the task's security group does not admit the load balancer's health-check traffic the target is still REGISTERED and reported with State 'unhealthy' and one of those reasons — registration and reachability are separate facts.
Emulator: the target group never lists the task at all. The check's own assertion is `AssertionError: target was never registered in the target group`, raised after its polling window with no state ever observed.
Why: the negative conclusion this check exists to draw — 'the ALB cannot reach the task because of the security group' — is unreachable, because the emulator's answer for a blocked target and for a service that never wired up is the same empty list. A client diagnosing an unreachable service is given no state and no reason code to act on, and the health-check evidence that would name the security group is absent.
Fix: the ECS task-registration hop into elbv2 (ecs service crate placing the task's target into elbv2 state) and the elbv2 target-health evaluator that must then report State 'unhealthy' with Reason 'Target.Timeout'/'Target.FailedHealthChecks' when the security groups do not admit the health-check traffic.
Traces: cloudwatch__ecs__elbv2__logs.alb-security-group-mismatch-blocks-health-checks.

## Evidence
Replay of suite simparity-behavioral-aws-v1: 202/554 passed; cloudwatch__ecs__elbv2__logs 0/19 (0%), 16 failed (3 unloadable). 15 of the 16 are marked OWNED; this one is not.

[mismatch] cloudwatch__ecs__elbv2__logs.alb-security-group-mismatch-blocks-health-checks:
  File "run.py", line 353, in <lambda>
  File "check_cloudwatch__ecs__elbv2__logs_alb_security_group_mismatch_blocks_health_checks.py", line 77, in run
    assert last_states, "target was never registered in the target group"
  AssertionError: target was never registered in the target group

The check builds an ECS service (image public.ecr.aws/docker/library/nginx:alpine) behind an ALB whose security group is deliberately not admitted by the task's security group, and then reads DescribeTargetHealth. Expected: at least one TargetHealthDescription exists, with State 'unhealthy' and Reason 'Target.Timeout' or 'Target.FailedHealthChecks'. Actual: no target description is ever returned (`last_states` is empty), so the check cannot assert the unhealthy reason.

Smithy consulted: elbv2 DescribeTargetHealth -> TargetHealthDescriptions; TargetHealthStateEnum and TargetHealthReasonEnum with the documented unhealthy reasons quoted above.

## Steps to Reproduce
1. Create a VPC with two subnets; create security group SG-ALB (ingress 80 from anywhere) and SG-TASK with NO ingress from SG-ALB.
2. Create an ECS cluster, task execution role and an awsvpc task definition serving HTTP on 80; create an elbv2 target group and an ALB (SG-ALB) with a listener forwarding to it.
3. ecs.create_service(desiredCount=1, loadBalancers=[{targetGroupArn, containerName, containerPort: 80}], networkConfiguration using SG-TASK); wait for the task to be RUNNING.
4. Poll elbv2.describe_target_health(TargetGroupArn=<tg>): AWS lists the task as a registered target with State 'unhealthy' and Reason 'Target.Timeout'/'Target.FailedHealthChecks'. The emulator returns no target descriptions at all.
5. Re-replay cloudwatch__ecs__elbv2__logs.alb-security-group-mismatch-blocks-health-checks.
