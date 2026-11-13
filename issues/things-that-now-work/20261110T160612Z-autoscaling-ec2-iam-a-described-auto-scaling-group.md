Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__iam.e2e-asgeni-secondary-eni-attach-failure-lifecycle
Issue-Key: autoscaling__ec2__iam.e2e-asgeni-secondary-eni-attach-failure-lifecycle|phase#5|_PhaseError|raise _PhaseError(
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# autoscaling__ec2__iam: a described Auto Scaling group's tags omit PropagateAtLaunch, so reading a group's tags raises KeyError

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
Cause 1 of 1: TagDescription entries returned for an Auto Scaling group carry no PropagateAtLaunch member.

AWS:      Smithy: TagDescription (com.amazonaws.autoscaling#TagDescription) has members ResourceId, ResourceType, Key, Value and PropagateAtLaunch (boolean, "Determines whether the tag is added to new instances as they are launched in the group"). Every tag DescribeAutoScalingGroups reports carries PropagateAtLaunch; a tag created without the flag stores and reports `false`, so the member is always present.
Emulator: the deploy phase of the ENI lifecycle check dies with `KeyError: 'PropagateAtLaunch'` while reading the tags of the group the stack created — the member is absent from the TagDescription entries.
Why:      PropagateAtLaunch is the whole point of an ASG tag: it decides whether launched instances inherit it. A client that reads `tag["PropagateAtLaunch"]` to decide inheritance (the SDK models it as always present) crashes, and tag-driven inventory over ASG-launched instances cannot be computed at all.
Fix:      the DescribeAutoScalingGroups response serializer / the group tag state for autoscaling — emit PropagateAtLaunch on every TagDescription, defaulting to false when the tag was created without it.
Traces:   autoscaling__ec2__iam.e2e-asgeni-secondary-eni-attach-failure-lifecycle (fails in phase 1/6 deploy).

Related but not covering: the open issue 20261102T104402Z-autoscaling-ec2-5-net-new-root-causes-described.md records the same missing member for checks under the autoscaling__ec2 heading; it does not carry this check's replay key, and this check is reported under autoscaling__ec2__iam.

## Evidence
Expected (AWS): each entry of the group's `Tags` is a TagDescription with ResourceId, ResourceType='auto-scaling-group', Key, Value and PropagateAtLaunch (false when not requested).
Actual (emulator):
  File "check_autoscaling__ec2__iam_e2e_asgeni_secondary_eni_attach_failure_lifecycle.py", line 268, in phase
  _PhaseError: [phase 1/6 deploy] KeyError: 'PropagateAtLaunch'

## Steps to Reproduce
1. Create an Auto Scaling group (directly or through a CloudFormation AWS::AutoScaling::AutoScalingGroup) with Tags=[{Key:'Customer-SG', Value:<id>}] and no PropagateAtLaunch.
2. DescribeAutoScalingGroups(AutoScalingGroupNames=[name]).
3. Read `group["Tags"][0]["PropagateAtLaunch"]` — the key does not exist; AWS reports `False`.
