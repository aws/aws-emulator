Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted
Issue-Key: autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# autoscaling__ec2__elbv2: the delete-target-group scale-out block scenario never reaches an observable step — the ASG/target-group lifecycle stalls for its whole budget

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetDesiredCapacity

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: a scale-out of an Auto Scaling group whose attached ELBv2 target group has been deleted out-of-band never produces any observable outcome — neither the successful pre-delete scale-out nor the post-delete refusal — and the check burns its entire budget without the emulator ever advancing the scenario.
AWS: Smithy (autoscaling) CreateAutoScalingGroup with TargetGroupARNs, SetDesiredCapacity, DescribeAutoScalingGroups -> AutoScalingGroups[].Instances[].LifecycleState; Smithy (elasticloadbalancingv2) DeleteTargetGroup and DescribeTargetHealth. On AWS the first scale-out launches an instance that reaches LifecycleState 'InService' and is registered into the attached target group; after the target group is deleted, the next scale-out's registration step fails and the group's scaling activity records a failure naming the missing target group ("target group not found" class), launching nothing; attaching a fresh target group lifts the block and the new instance registers there. The requirement is that each of those three phases reaches a terminal, observable state.
Emulator: the check is killed by its own ceiling — "Check timed out after 60.0s" — i.e. no phase of the lifecycle settles. Nothing in the group transitions to a terminal LifecycleState and no scaling-activity outcome is published for the post-delete attempt, so neither the happy path nor the refusal is reachable.
Why: a client cannot distinguish "the target group is gone so this scale-out was refused" from "the scale-out is still in flight". Capacity-management logic that waits for InService, or that reacts to a failed registration, waits forever; the negative class AWS surfaces (a refusal that names the missing target group) is never observable at all.
Fix: the Auto Scaling group lifecycle component — the instance-launch/registration path that must drive instances to a terminal LifecycleState and record a scaling-activity failure when the attached target group no longer resolves in ELBv2 — together with the ELBv2 registration call it makes.
Related open filings that do NOT carry this replay key: 20261107T075601Z-autoscaling-ec2-elbv2-auto-scaling-elbv2-asg-instances-never-register-with.md and 20261109T153103Z-autoscaling-ec2-elbv2-2-net-new-causes-putscalingpolicy.md describe adjacent ASG/target-group registration gaps, but the suite did not mark this check OWNED by either, so it survives as a live failure.
Traces: autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator; service heading autoscaling__ec2__elbv2 10/60 (17%), 50 failed (all but this one OWNED or ALREADY FILED).

[mismatch] autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted
  Error: Check timed out after 60.0s

The check's own body: create a launch template, create a target group, CreateAutoScalingGroup(min=1,max=3,desired=1, TargetGroupARNs=[tg], HealthCheckType='EC2'), then _wait_for_inservice_instances(count=1) — which is where the budget is consumed.

Expected (AWS): the first instance reaches LifecycleState 'InService' and appears in DescribeTargetHealth for the attached group; after DeleteTargetGroup, the next SetDesiredCapacity records a failed scaling activity naming the missing target group and launches nothing; after attaching a fresh target group the new instance registers there.
Actual (emulator): no instance ever reaches a terminal LifecycleState; the check never gets past the first wait.

Smithy consulted (model services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01 and services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10): CreateAutoScalingGroup input TargetGroupARNs; DescribeAutoScalingGroups output AutoScalingGroups[].Instances[].LifecycleState; DescribeScalingActivities output Activities[].{StatusCode,StatusMessage}; elbv2 DeleteTargetGroup, DescribeTargetHealth.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the check id in trace_ids reproduces.
2. Create an EC2 launch template and an ELBv2 target group in the default VPC.
3. CreateAutoScalingGroup(MinSize=1, MaxSize=3, DesiredCapacity=1, TargetGroupARNs=[<tg>], HealthCheckType='EC2').
4. Poll DescribeAutoScalingGroups for an instance with LifecycleState 'InService'. Expected: one instance InService and visible in elbv2 DescribeTargetHealth. Actual: nothing reaches a terminal state within the budget.
5. (Unreachable today) DeleteTargetGroup, then SetDesiredCapacity to 2 and read DescribeScalingActivities: expect a failed activity naming the missing target group and no new instance.
6. Re-replay the id in trace_ids to confirm the fix.
