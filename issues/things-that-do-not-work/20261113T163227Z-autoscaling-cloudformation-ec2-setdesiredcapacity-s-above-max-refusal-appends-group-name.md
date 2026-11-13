Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__ec2.cfn-autoscaling-group-in-place-max-size-retained-replacement-enforces-bounds-then-not-found
Issue-Key: autoscaling__cloudformation__ec2.cfn-autoscaling-group-in-place-max-size-retained-replacement-enforces-bounds-then-not-found|_assert_bounded#25|AssertionError|assert got == _above_max(max_size + 1, max_size), (
Issue-Commit: b5b438418db974c5623067148485e4df5aeb5fcd

# SetDesiredCapacity's above-max refusal appends ": <group-name>" instead of ending with AWS's "for the AutoScalingGroup."

> _Note: this issue was identified against the code base at commit `b5b438418db974c5623067148485e4df5aeb5fcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetDesiredCapacity

## Description
One net-new cause.

Cause 1 of 1: SetDesiredCapacity above a group's MaxSize is refused with the right status and code but a re-worded message. Failing check: autoscaling__cloudformation__ec2.cfn-autoscaling-group-in-place-max-size-retained-replacement-enforces-bounds-then-not-found.

AWS: HTTP 400, awsQuery ErrorResponse, Error.Code ValidationError, Error.Message 'New SetDesiredCapacity value <requested> is above max value <MaxSize> for the AutoScalingGroup.' The fixed text is 'New SetDesiredCapacity value ' + ' is above max value ' + ' for the AutoScalingGroup.'; the only variable parts are the capacity from the request and the group's stored MaxSize, and the sentence ENDS at that period — the group's name is never part of it. Smithy models SetDesiredCapacity with only ScalingActivityInProgressFault and ResourceContentionFault, so a capacity-bound refusal is the service-common ValidationError and the sentence is recorded behavior only (it is what the replayed check asserts).

Emulator: the same HTTP 400 and Error.Code ValidationError, but Error.Message '... for the AutoScalingGroup: <group-name>' — AWS's terminating period is replaced by ': ' plus the Auto Scaling group name. The refusal is otherwise correct (the call changes nothing, and SetDesiredCapacity 0 is accepted afterwards).

Why: ValidationError is the generic code for every Auto Scaling argument refusal, so the message is the only machine-visible statement of WHICH rule failed. A caller or audit pipeline matching AWS's sentence byte-exactly never recognises the rejection, and because the appended group name is a per-run value the emulator's text is different on every run, so it cannot even be matched by a fixed string of its own.

Fix: the message builder in the autoscaling crate's SetDesiredCapacity handler, in the above-MaxSize branch of its bound check — emit AWS's sentence and stop at 'for the AutoScalingGroup.'

Traces: autoscaling__cloudformation__ec2.cfn-autoscaling-group-in-place-max-size-retained-replacement-enforces-bounds-then-not-found. The same wording defect is described, for OTHER check ids, by open issues 20261107T081710Z-autoscaling-auto-scaling-capacity-bound-refusals-6-net-new.md (its Cause 3) and 20261107T075601Z-autoscaling-ec2-events-sqs-asg-lifecycle-events-never-reach-eventbridge-sqs-and.md; neither carries this check's replay key, which exercises the bound on a CloudFormation-provisioned group (including the retained replacement twin), so it is filed here.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (810/2121 checks passed; the emulator is up and answering).

autoscaling__cloudformation__ec2.cfn-autoscaling-group-in-place-max-size-retained-replacement-enforces-bounds-then-not-found [mismatch]:
  _assert_bounded(autoscaling, first, 1) -> assert got == _above_max(max_size + 1, max_size)
  AssertionError: SetDesiredCapacity 2 on sim2real-cfnasg-a-8ad0f3e0cab9 answered ('ValidationError', 'New SetDesiredCapacity value 2 is above max value 1 for the AutoScalingGroup: sim2real-cfnasg-a-8ad0f3e0cab9')

The expectation the check encodes is ('ValidationError', 'New SetDesiredCapacity value 2 is above max value 1 for the AutoScalingGroup.'). 2 and 1 are this run's requested capacity and the group's MaxSize; 'sim2real-cfnasg-a-8ad0f3e0cab9' is a per-run generated group name and is not part of AWS's message at all. Error.Code (ValidationError) is correct; only the message text diverges, by the ': <group-name>' suffix replacing the terminating period.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay autoscaling__cloudformation__ec2.cfn-autoscaling-group-in-place-max-size-retained-replacement-enforces-bounds-then-not-found.
2. By hand: create an Auto Scaling group (directly or through a CloudFormation AWS::AutoScaling::AutoScalingGroup with a valid launch template) with MinSize=0, MaxSize=1, DesiredCapacity=0.
3. POST Action=SetDesiredCapacity with AutoScalingGroupName=<group> and DesiredCapacity=2. Expect HTTP 400, Error.Code ValidationError, Error.Message exactly 'New SetDesiredCapacity value 2 is above max value 1 for the AutoScalingGroup.'
4. The emulator answers the same status and code with 'New SetDesiredCapacity value 2 is above max value 1 for the AutoScalingGroup: <group-name>'.
5. Confirm the refused call left the group untouched and that SetDesiredCapacity 0 is still accepted (both hold today), so only the message is at fault.
