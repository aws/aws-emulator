Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2.asgeni-asg-desired-above-max-rejected autoscaling__ec2.asghc-asg-instance-inherits-template-sg-imdsv2-and-group-tag autoscaling__ec2.asghc-create-asg-with-a-template-sg-from-another-vpc-rejected
Issue-Key: autoscaling__ec2.asgeni-asg-desired-above-max-rejected|run#10|AssertionError|assert got == ("ValidationError",
Issue-Key: autoscaling__ec2.asghc-asg-instance-inherits-template-sg-imdsv2-and-group-tag|run#21|AssertionError|assert tagmap.get("aws:ec2launchtemplate:id") == lt["id"], (
Issue-Key: autoscaling__ec2.asghc-create-asg-with-a-template-sg-from-another-vpc-rejected|run#12|AssertionError|assert got == ("ValidationError", "One or more security groups in the launch template are "
Issue-Commit: 965b3053fd6a88603311dd441a6858d4fb538678

# Auto Scaling: 3 net-new causes — SetDesiredCapacity above-max message appends the group name, launched instances carry no launch-template tags, cross-VPC template-SG refusal text

> _Note: this issue was identified against the code base at commit `965b3053fd6a88603311dd441a6858d4fb538678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetDesiredCapacity

## Description
Three net-new root causes in the Auto Scaling surface exercised by the autoscaling__ec2 behavioral checks. Each cause is independent and must be closed separately.

Cause 1 of 3: SetDesiredCapacity's above-max refusal message appends the group name.
AWS: SetDesiredCapacity with DesiredCapacity above the group's MaxSize is refused with HTTP 400, error code ValidationError and the message "New SetDesiredCapacity value <desired> is above max value <max> for the AutoScalingGroup." The fixed text is "New SetDesiredCapacity value " / " is above max value " / " for the AutoScalingGroup."; the only variable parts are the requested desired capacity and the group's MaxSize, both taken from the request and the stored group. The group name is NOT part of the message. Smithy (autoscaling model): SetDesiredCapacity declares ScalingActivityInProgressFault and ResourceContentionFault only, so the bound check is a service validation whose wording is the documented/recorded behavior.
Emulator: answers ValidationError with "New SetDesiredCapacity value 2 is above max value 1 for the AutoScalingGroup: <group-name>" — the sentence-ending period is replaced by ": " plus the group name.
Why: a client that matches on the exact refusal text (or logs it for comparison) sees a message real AWS never emits; the trailing group name also leaks a per-run value into what callers treat as fixed text.
Fix: the SetDesiredCapacity handler's bound validation and its message template. Note: open issues 20261113T163227Z-autoscaling-cloudformation-ec2-setdesiredcapacity-s-above-max-refusal-appends-group-name.md, 20261107T081710Z-autoscaling-auto-scaling-capacity-bound-refusals-6-net-new.md and 20261107T075601Z-autoscaling-ec2-events-sqs-asg-lifecycle-events-never-reach-eventbridge-sqs-and.md describe the same wording under other service tokens; this check (autoscaling__ec2) was not marked owned by any of them.
Traces: autoscaling__ec2.asgeni-asg-desired-above-max-rejected (the UpdateAutoScalingGroup leg of the same check passes; only the SetDesiredCapacity leg diverges).

Cause 2 of 3: an instance launched by an Auto Scaling group carries no aws:ec2launchtemplate:* tags.
AWS: when a group launches from a launch template, Auto Scaling tags each instance with aws:autoscaling:groupName=<group name>, aws:ec2launchtemplate:id=<lt-id> and aws:ec2launchtemplate:version=<resolved version>, in addition to the group's PropagateAtLaunch tags. DescribeInstances reports all of them. (recorded behavior only — the tag keys are service-applied, not modeled.)
Emulator: the instance's tag set is only {aws:autoscaling:groupName, <propagated tag>}; the aws:ec2launchtemplate:id and aws:ec2launchtemplate:version tags are absent.
Why: tooling that attributes a running instance back to the template/version that produced it (drift detection, cost attribution, instance-refresh reasoning) finds no template reference and concludes the instance was not template-launched.
Fix: the Auto Scaling launch path that stamps service tags onto the instances it creates (and EC2's tag read path for those instances).
Traces: autoscaling__ec2.asghc-asg-instance-inherits-template-sg-imdsv2-and-group-tag (type/AMI/subnet/SG/IMDS options all round-trip; only the template tags are missing).

Cause 3 of 3: CreateAutoScalingGroup's cross-VPC launch-template security-group refusal uses emulator-authored wording.
AWS: creating a group whose launch template names a security group from a different VPC than the group's VPCZoneIdentifier subnets is refused with HTTP 400, ValidationError and the message "One or more security groups in the launch template are not linked to the VPCs configured in the Auto Scaling group" (entirely fixed text, no request values).
Emulator: answers ValidationError with "One or more security groups in the launch template are not linked to the VPC that the Auto Scaling group will launch instances in." — correct code and refusal, different sentence.
Why: a client asserting on, or translating, the documented message does not recognise the refusal.
Fix: the CreateAutoScalingGroup validation that cross-checks launch-template security groups against the group's subnets — its message template.
Traces: autoscaling__ec2.asghc-create-asg-with-a-template-sg-from-another-vpc-rejected (the positive control with the subnet's own group is accepted).

## Evidence
Cause 1 — expected ('ValidationError', 'New SetDesiredCapacity value 2 is above max value 1 for the AutoScalingGroup.'); got ('ValidationError', 'New SetDesiredCapacity value 2 is above max value 1 for the AutoScalingGroup: sim2real-asgeni-asg-35f779985395').
Cause 2 — AssertionError: the instance does not name its launch template: {'aws:autoscaling:groupName': 'sim2real-asghc-e36aca53d789', 'sim2real': 'asghc'} (expected aws:ec2launchtemplate:id == the group's launch-template id).
Cause 3 — expected ('ValidationError', 'One or more security groups in the launch template are not linked to the VPCs configured in the Auto Scaling group'); got ('ValidationError', 'One or more security groups in the launch template are not linked to the VPC that the Auto Scaling group will launch instances in.').

## Steps to Reproduce
1. Cause 1: CreateAutoScalingGroup with MinSize=0, MaxSize=1, DesiredCapacity=0 from a launch template, then SetDesiredCapacity(DesiredCapacity=2). Compare the ValidationError message to AWS's.
2. Cause 2: scale the same group to DesiredCapacity=1, wait for InService, then DescribeInstances on the launched instance and read its Tags.
3. Cause 3: create a second VPC, a security group in it and a launch template naming that group; CreateAutoScalingGroup with VPCZoneIdentifier pointing at the first VPC's subnet and that template. Compare the ValidationError message.
Replay: autoscaling__ec2.asgeni-asg-desired-above-max-rejected, autoscaling__ec2.asghc-asg-instance-inherits-template-sg-imdsv2-and-group-tag, autoscaling__ec2.asghc-create-asg-with-a-template-sg-from-another-vpc-rejected.
