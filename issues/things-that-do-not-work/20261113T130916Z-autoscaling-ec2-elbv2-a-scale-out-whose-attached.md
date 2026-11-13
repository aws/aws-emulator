Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted
Issue-Key: autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted|run#24|Timeout|time.sleep(stall_interval_s)
Issue-Commit: 4370547da342f3cac1e3d2a524b52e751b2d7b18

# autoscaling__ec2__elbv2: a scale-out whose attached target group was deleted out-of-band is never cancelled — no Failed scaling activity naming the target group is ever recorded

> _Note: this issue was identified against the code base at commit `4370547da342f3cac1e3d2a524b52e751b2d7b18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingActivities

## Description
Cause 1 of 1: after the target group attached to an Auto Scaling group is deleted in ELBv2, the group's next scale-out is still treated as a success — the emulator never records the Failed scaling activity AWS records, and the scenario never reaches a verdict.
AWS: Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json): SetDesiredCapacity input {AutoScalingGroupName required, DesiredCapacity required, HonorCooldown}, empty output, errors ScalingActivityInProgressFault / ResourceContentionFault — so the scale-out REQUEST is accepted and the outcome is reported asynchronously through DescribeScalingActivities -> Activities (required list) whose members carry StatusCode and StatusMessage/Description. Recorded behavior for the outcome: a group whose TargetGroupARNs name a target group that no longer exists cannot register the instance it launches, so the activity ends with StatusCode 'Failed' and a StatusMessage/Description that names the target group (the text contains 'target group'); the desired instance does not stay InService, and attaching a fresh target group lifts the block so the next launch registers there.
Emulator: no Failed activity ever appears for the scale-out issued after DeleteTargetGroup — the poll for an activity with StatusCode 'Failed' whose message mentions the target group never becomes satisfiable, and the check exhausts its budget (the only recorded symptom is 'Check timed out after 60.0s'; the emulator returned no refusal and no failed activity at any point).
Why: a client's only window into an asynchronous scale-out is the activity record. With a dangling target group the group is silently broken: capacity appears to grow while nothing is reachable behind the load balancer, and the operator gets no Failed activity, no message naming the deleted target group and therefore no signal to repoint the group. Remediation flows that wait for the failure, then swap in a new target group, never trigger.
Fix: the launch/registration path in the autoscaling service's scaling-activity recording — resolve each TargetGroupARN of the group at launch time and, when it does not resolve, record the activity with StatusCode 'Failed' and a StatusMessage naming that target group instead of recording a success.
Traces: autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted.

Related open filings that do NOT carry this check and therefore do not cover it: 20261111T010150Z-autoscaling-ecs-elbv2-asg-launch-into-a-target-group-in-a.md (the mismatched-VPC variant, filed under autoscaling__ecs__elbv2) and 20261111T102954Z-autoscaling-scaling-activities-do-not-record-real-launch.md (failed launches recorded Successful, filed under autoscaling). The replay marked this cluster neither OWNED nor ALREADY FILED.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the booted emulator.

[mismatch] autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted:
  Error: Check timed out after 60.0s

The check creates a launch template, a target group and an ASG (min 1 / max 3 / desired 1, TargetGroupARNs=[tg], HealthCheckType='EC2'), scales out once as a positive control, asserts both instances are registered in the target group, then calls elbv2 DeleteTargetGroup and SetDesiredCapacity(DesiredCapacity=3, HonorCooldown=False). It then polls DescribeScalingActivities for an activity with StatusCode == 'Failed' whose StatusMessage/Description contains 'target group' and asserts: "expected a Failed scaling activity naming the deleted target group, got none". That condition is never satisfied and the body is cut off at the harness budget, so the emulator produced no such activity. NOTE: the only diagnostic this check yields is the budget expiry — no status, code or message was returned to compare, so the divergence is grounded in the absent Failed activity rather than in a wrong response body. Target-group ARNs, instance ids and names are generated per run.

## Steps to Reproduce
1. Clone the repo, boot the emulator and replay the behavioral suite; autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted reproduces.
2. Create an ELBv2 target group in the default VPC and an Auto Scaling group with TargetGroupARNs=[that arn], MinSize=1, MaxSize=3, DesiredCapacity=1, HealthCheckType='EC2'; wait for the instance to be InService and registered in the target group.
3. Call elbv2 DeleteTargetGroup on that arn, then autoscaling SetDesiredCapacity(DesiredCapacity=3, HonorCooldown=False).
4. Poll DescribeScalingActivities for the group. Expect an activity with StatusCode 'Failed' whose StatusMessage/Description names the deleted target group, and no new instance left InService; the emulator records no Failed activity at all.
5. Attach a fresh target group and confirm the next scale-out succeeds and registers there.
6. Re-replay autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted to confirm the fix.
