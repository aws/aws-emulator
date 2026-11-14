Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs__elbv2.ecs-service-missing-elb-service-linked-role-negative
Issue-Key: cloudformation__ecs__elbv2.ecs-service-missing-elb-service-linked-role-negative|_poll_until#7|Timeout|last = probe()
Issue-Commit: 1d457c6418d8d9996f08d1176214615fe9686e31

# cloudformation__ecs__elbv2: the EC2-launch-type CreateService service-role/elbv2-permission scenario reaches no verdict — the check is cut off at the harness cap

> _Note: this issue was identified against the code base at commit `1d457c6418d8d9996f08d1176214615fe9686e31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: the EC2-launch-type CreateService permission-boundary scenario never becomes observable — it produces no verdict and is killed at the harness cap, so neither its refusal nor its positive control is ever exercised.
AWS: recorded behavior only for the scenario's progress; the asserted refusal is a requirement. The check launches an ECS-optimized EC2 instance with ECS_CLUSTER user data, waits (its own budget: 240 s) for DescribeContainerInstances to show it ACTIVE, creates an ALB/target group, and then calls CreateService with launchType EC2 and a `role` that lacks elasticloadbalancing permissions. On AWS that call is refused with HTTP 400, code InvalidParameterException and the fixed message "Unable to assume role and validate the specified targetGroupArn. Please verify that the ECS service role being passed has the proper permissions." (fixed text for this class — it carries no request- or state-derived values), no service appears in ListServices and the target group stays empty; with an elbv2-capable role the identical call registers exactly one target.
Emulator: the check is cut off with "Check timed out after 60.0s" having reached no verdict at all — the scenario makes no observable progress past its early EC2 container-instance registration wait, so the refusal above, the effect-absent reads and the positive control are all unreachable.
Why: a client cannot exercise any EC2-launch-type service creation that depends on a registered container instance, and the service-role permission boundary (the refusal that protects it from a silently role-less service) can neither be observed nor relied on.
Fix: the container-instance registration state this flow waits on (a launched ECS-optimized EC2 host must become an observable ACTIVE container instance of its cluster) and, behind it, CreateService's EC2-launch-type `role`/targetGroupArn validation that must produce the InvalidParameterException above.
Traces: cloudformation__ecs__elbv2.ecs-service-missing-elb-service-linked-role-negative.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator.

[mismatch] cloudformation__ecs__elbv2.ecs-service-missing-elb-service-linked-role-negative
  Error: Check timed out after 60.0s

The check body (from the replay dump) shows the first long wait is _wait_container_instance_active(ctx, cluster_arn, timeout_s=240) after ec2.run_instances with `echo ECS_CLUSTER=<cluster> >> /etc/ecs/ecs.config` user data; only after that does it assert:
  code == 'InvalidParameterException' and message == 'Unable to assume role and validate the specified targetGroupArn. Please verify that the ECS service role being passed has the proper permissions.'
  then: the rejected service name absent from ListServices, DescribeTargetHealth empty, and — with an elbv2-capable role — exactly one registered target.

Expected (AWS): the whole scenario completes, with HTTP 400 InvalidParameterException and that exact fixed message for the permission-less role. Actual (emulator): no verdict at all; the check is killed at the 60 s cap with no progress.

## Steps to Reproduce
1. Clone the repo and replay suite simparity-behavioral-aws-v1; the check id in trace_ids reproduces.
2. CreateCluster; launch an ECS-optimized EC2 instance (instance profile with AmazonEC2ContainerServiceforEC2Role) whose user data sets ECS_CLUSTER to that cluster; poll DescribeContainerInstances until it is ACTIVE.
3. RegisterTaskDefinition (EC2, bridge), create an ALB, a target group of type instance and a listener.
4. CreateService with launchType EC2, loadBalancers naming the target group, and `role` set to an ecs.amazonaws.com-trusting role WITHOUT elasticloadbalancing permissions.
5. Expected: HTTP 400 InvalidParameterException, message 'Unable to assume role and validate the specified targetGroupArn. Please verify that the ECS service role being passed has the proper permissions.'; the service absent from ListServices and DescribeTargetHealth empty. Then repeat with an elbv2-capable role and observe exactly one registered target.
6. Actual: the scenario never gets that far — the check reaches no verdict and is cut off at the harness cap.
7. Re-replay the id in trace_ids to confirm the fix.
