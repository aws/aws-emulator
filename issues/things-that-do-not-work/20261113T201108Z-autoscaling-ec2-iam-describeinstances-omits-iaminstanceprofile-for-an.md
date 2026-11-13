Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__iam.e2e-asgeni-secondary-eni-attach-failure-lifecycle
Issue-Key: autoscaling__ec2__iam.e2e-asgeni-secondary-eni-attach-failure-lifecycle|phase#6|_PhaseError|raise _PhaseError(
Issue-Commit: 751e4cd59e0742391b3a649cdf34c459ab8203ec

# autoscaling__ec2__iam: DescribeInstances omits IamInstanceProfile for an instance launched from a launch template that declares LaunchTemplateData.IamInstanceProfile

> _Note: this issue was identified against the code base at commit `751e4cd59e0742391b3a649cdf34c459ab8203ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstances

## Description
Cause 1 of 1: an instance launched by an Auto Scaling group whose launch template declares LaunchTemplateData.IamInstanceProfile = {Arn: <instance profile ARN>} is reported by DescribeInstances with NO IamInstanceProfile member at all.
AWS: Smithy: com.amazonaws.ec2#Instance carries the member IamInstanceProfile (com.amazonaws.ec2#IamInstanceProfile, members Arn and Id, doc: "The IAM instance profile associated with the instance, if applicable"); the launch-template read shape carries LaunchTemplateData.IamInstanceProfile.Arn, which Auto Scaling passes through to the RunInstances it performs. Recorded behavior: when the resolved launch-template version names an instance profile, the launched instance's DescribeInstances payload contains IamInstanceProfile with Arn equal to that profile's ARN (the ARN value itself is per-run state, taken from the stack output / the profile the template names — the requirement is that the member is present and equals the template's ARN).
Emulator: the key is absent from the Instance structure, so the client's `inst["IamInstanceProfile"]` raises KeyError: 'IamInstanceProfile'. The same instance's SubnetId, SecurityGroups, BlockDeviceMappings and aws:autoscaling:groupName tag were already asserted and did NOT diverge, so the launch itself happened — only the profile association is missing from the launch path and/or the DescribeInstances serializer.
Why: the instance-profile ARN on a running instance is the only link from an instance to the IAM role it runs as. Without it, every role-attribution flow breaks: "which role does this instance use", console/CLI troubleshooting of denied instance calls, SSM/SSH connectivity diagnosis, and any policy simulation that starts from the instance's profile. A client reading the documented member crashes rather than reading a wrong value.
Fix: the Auto Scaling / RunInstances launch path must apply the resolved launch-template version's IamInstanceProfile to the created instance's state, and the DescribeInstances serializer must emit Instance.IamInstanceProfile (Arn, Id) whenever an instance has one. Related but distinct: open issue 20261106T015719Z-autoscaling-ec2-auto-scaling-launch-from-a-launch-template-loses.md covers other launch-template data lost on launch (security groups, TagSpecifications) and does not mention IamInstanceProfile.
Traces: autoscaling__ec2__iam.e2e-asgeni-secondary-eni-attach-failure-lifecycle, phase 2/6 pre_invoke, assert at check line 268/`inst["IamInstanceProfile"]["Arn"]` (check line 42845 region of the replay report).

## Evidence
Check [error] autoscaling__ec2__iam.e2e-asgeni-secondary-eni-attach-failure-lifecycle
  Expected (AWS): in phase pre_invoke, after the CloudFormation stack's group (launch template declares IamInstanceProfile.Arn = Fn::GetAtt HelperLTProfile.Arn) is scaled to one live instance, ec2 DescribeInstances on that instance returns Instance.IamInstanceProfile.Arn == the stack's InstanceProfileArn output.
  Actual (emulator): _PhaseError: [phase 2/6 pre_invoke] KeyError: 'IamInstanceProfile' — the Instance structure has no IamInstanceProfile member. The preceding assertions on the same instance (SubnetId == the helper subnet, SecurityGroups == [the helper group], root volume present, aws:autoscaling:groupName tag present) passed.

## Steps to Reproduce
1. Create an IAM role assumable by ec2.amazonaws.com and an instance profile holding it (or deploy a stack that does), and note the profile ARN.
2. ec2 CreateLaunchTemplate with LaunchTemplateData {ImageId: <AL2023 AMI>, InstanceType: t3.micro, IamInstanceProfile: {Arn: <profile ARN>}, SecurityGroupIds: [<sg>], MetadataOptions: {HttpTokens: required}}.
3. autoscaling CreateAutoScalingGroup(MinSize=0, MaxSize=1, DesiredCapacity=0, VPCZoneIdentifier=<subnet>, LaunchTemplate={LaunchTemplateId: <lt>, Version: <latest>}), then scale it to DesiredCapacity 1 and wait for the instance.
4. ec2 DescribeInstances(InstanceIds=[<the launched instance>]) and read Reservations[0].Instances[0].IamInstanceProfile.
   Observed: the member is absent (KeyError in any client that reads it). Expected: {Arn: <profile ARN>, Id: <profile id>}.
