Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2.asgeni-scale-in-to-zero-eni-not-found-by-id-and-unusable
Issue-Key: autoscaling__ec2.asgeni-scale-in-to-zero-eni-not-found-by-id-and-unusable|run#19|AssertionError|assert (attachment["InstanceId"], attachment["Status"]) == (iid, "attached"), (
Issue-Commit: 751e4cd59e0742391b3a649cdf34c459ab8203ec

# autoscaling__ec2: the primary ENI of an ASG-launched instance reads Attachment.Status 'attaching' where AWS reports 'attached'

> _Note: this issue was identified against the code base at commit `751e4cd59e0742391b3a649cdf34c459ab8203ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeNetworkInterfaces

## Description
Cause 1 of 1: once an Auto Scaling group has launched an instance and DescribeInstances reports that instance's NetworkInterfaces, DescribeNetworkInterfaces on the same interface id reports its Attachment.Status as `attaching` instead of `attached`.
AWS: Smithy: com.amazonaws.ec2#NetworkInterfaceAttachment.Status targets com.amazonaws.ec2#AttachmentStatus, an enum whose values are exactly attaching | attached | detaching | detached (doc: "The attachment state"); the same structure carries AttachTime, AttachmentId, DeviceIndex 0, NetworkCardIndex, InstanceId, InstanceOwnerId and DeleteOnTermination. Recorded behavior: an instance's primary interface (DeviceIndex 0) is attached as part of the launch — by the time DescribeInstances lists the instance's NetworkInterfaces, an independent DescribeNetworkInterfaces on that interface id reports Attachment.Status == "attached" and Attachment.InstanceId == the instance id. `attaching` is the transient state of an in-flight AttachNetworkInterface, not the state of a launched instance's primary interface.
Emulator: the interface is created with the right identity (InstanceId, DeviceIndex 0, NetworkCardIndex 0, DeleteOnTermination true, an AttachTime and an eni-attach-... AttachmentId) but Status stays `attaching` at the point AWS already reports `attached`; the instance is already reported with its NetworkInterfaces, so the read is not racing a pending launch. Hypothesis: the attachment is materialised in its initial state and the transition to `attached` is never applied.
Why: every client that gates on the documented attachment state — waiters, CDK/CloudFormation style polls, ENI-health or secondary-interface tooling, and any diagnosis that distinguishes "attach in flight" from "attached" — reads a permanently in-flight interface for a fully launched instance, so it either waits forever or concludes the attachment failed.
Fix: the EC2 network-interface state in the ASG/RunInstances launch path (and the DescribeNetworkInterfaces serializer that reads it) must settle the primary interface's attachment to `attached` once the instance's interface is attached, rather than leaving the initial `attaching`.
Traces: autoscaling__ec2.asgeni-scale-in-to-zero-eni-not-found-by-id-and-unusable (assert at check line 57).

## Evidence
Check [mismatch] autoscaling__ec2.asgeni-scale-in-to-zero-eni-not-found-by-id-and-unusable
  Expected (AWS): after UpdateAutoScalingGroup raises the group to DesiredCapacity 1 and DescribeInstances reports the instance with exactly one NetworkInterface, DescribeNetworkInterfaces(NetworkInterfaceIds=[<that eni>])[0].Attachment reads (InstanceId=<the instance id>, Status="attached").
  Actual (emulator): AssertionError: "<eni> should read attached to <instance>; got {'AttachTime': ..., 'AttachmentId': 'eni-attach-...', 'DeleteOnTermination': True, 'DeviceIndex': 0, 'NetworkCardIndex': 0, 'InstanceId': '<the instance id>', 'InstanceOwnerId': '...', 'Status': 'attaching'}" — the InstanceId is correct, only Status diverges ('attaching' instead of 'attached'). The ids/timestamps in that payload are per-run values; the requirement is Status == "attached".

## Steps to Reproduce
1. Create a VPC, a subnet and a security group; ec2 CreateLaunchTemplate with LaunchTemplateData {ImageId: <AL2023 AMI>, InstanceType: t3.micro, SecurityGroupIds: [<sg>], MetadataOptions: {HttpTokens: required}}.
2. autoscaling CreateAutoScalingGroup(MinSize=0, MaxSize=1, DesiredCapacity=0, VPCZoneIdentifier=<subnet>, LaunchTemplate={LaunchTemplateId: <lt>, Version: "$Latest"}).
3. autoscaling UpdateAutoScalingGroup(MinSize=1, MaxSize=1, DesiredCapacity=1) and poll DescribeAutoScalingGroups until one instance is Pending/InService.
4. ec2 DescribeInstances(InstanceIds=[<that instance>]) and poll until it reports NetworkInterfaces; take the single NetworkInterfaceId.
5. ec2 DescribeNetworkInterfaces(NetworkInterfaceIds=[<that eni>]) and read NetworkInterfaces[0].Attachment.Status.
   Observed: "attaching". Expected: "attached".
