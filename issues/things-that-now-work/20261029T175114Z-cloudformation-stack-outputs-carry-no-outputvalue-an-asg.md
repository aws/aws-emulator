Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected cloudformation__ec2.deploy-ec2-1-overlapping-subnet-cidr-rejected-and-rolled-back cloudformation__ec2__ssm.cli-script-cfn-update-stack-security-group-blocks-ssm-negative
Issue-Key: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal|_deploy_cfn#9|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected|_in_service_ids#18|AssertionError|assert groups, f"auto scaling group {asg_name!r} not found"
Issue-Key: cloudformation__ec2.deploy-ec2-1-overlapping-subnet-cidr-rejected-and-rolled-back|run#15|AssertionError|assert [e["LogicalResourceId"] for e in failed] == ["Overlap"], (
Issue-Key: ssm|DescribeInstanceInformation|InvalidInstanceId
Issue-Commit: abd75e982d55cd9e91541cb3d125feb2d8edb60f

# CloudFormation: stack Outputs carry no OutputValue, an ASG the stack creates is never visible, a failed create reports an extra stack-level CREATE_FAILED event, and an EC2 instance's PhysicalResourceId is its logical name (4 causes)

> _Note: this issue was identified against the code base at commit `abd75e982d55cd9e91541cb3d125feb2d8edb60f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Four root causes in the CloudFormation crate, all reached through EC2/Auto Scaling stacks.

Cause 1 of 4: DescribeStacks returns Outputs entries without an OutputValue member.
AWS: every Output in a stack that reached CREATE_COMPLETE carries OutputKey AND OutputValue (the resolved value); clients index OutputValue directly.
Emulator: the OutputValue key is absent, so a client reading it raises KeyError and cannot consume any stack output.
Why: stack outputs are the only supported way to get a created resource's identifier out of a stack; without OutputValue every downstream step of every deploy scenario is blocked.
Fix: the CloudFormation Output resolution/serialization in DescribeStacks. The open issue 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md carries other checks; it does not carry this replay key.
Traces: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal.

Cause 2 of 4: an AWS::AutoScaling::AutoScalingGroup a stack creates is not visible through Auto Scaling afterwards.
AWS: after the stack reaches CREATE_COMPLETE, DescribeAutoScalingGroups with the group's physical name returns exactly one group.
Emulator: DescribeAutoScalingGroups for the name the stack reports returns zero groups, so the replacing-update/failed-signal contract cannot be exercised at all.
Why: a client that deploys an ASG by template and then reads it through the Auto Scaling API finds nothing, so update and rollback behaviour is unobservable.
Fix: the CloudFormation resource provider for AWS::AutoScaling::AutoScalingGroup — it must create the group in Auto Scaling state under the name it reports as the physical id.
Traces: autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected.

Cause 3 of 4: a create that fails on one resource emits an extra stack-level CREATE_FAILED event.
AWS: when one resource fails, the CREATE_FAILED events name exactly the failing resource(s); the stack-level record of the failure is the stack status reason, not an additional CREATE_FAILED event for the stack itself.
Emulator: the event stream contains BOTH the failing resource's CREATE_FAILED (correctly worded: `Resource handler returned message: "The CIDR <cidr> conflicts with another subnet (<subnetId>)" (RequestToken: <token>, HandlerErrorCode: ResourceConflict)`) AND a second CREATE_FAILED whose LogicalResourceId is the stack name with reason `The following resource(s) failed to create: [<LogicalId>]. `.
Why: a client that selects CREATE_FAILED events to report which resource broke gets the stack itself in that list and misattributes the failure.
Fix: the CloudFormation event recorder — the stack-level failure must not be emitted as a resource CREATE_FAILED event.
Traces: cloudformation__ec2.deploy-ec2-1-overlapping-subnet-cidr-rejected-and-rolled-back.

Cause 4 of 4: the PhysicalResourceId of an AWS::EC2::Instance is the stack-qualified logical name instead of the instance id.
AWS: the PhysicalResourceId of an AWS::EC2::Instance is its `i-...` id; downstream APIs (SSM DescribeInstanceInformation, EC2 DescribeInstances) accept it directly.
Emulator: reports a name of the form `<stack-name>-<LogicalId>-<suffix>`, so the next call refuses it on the id pattern — SSM DescribeInstanceInformation answers `InvalidInstanceId` / `Value at 'instanceId' failed to satisfy constraint: Member must satisfy regular expression pattern: (^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)` and EC2 DescribeInstances answers `InvalidInstanceID.Malformed` / `Invalid id: "<that name>"`.
Why: every cross-service step that takes an instance id from a stack resource is broken, and the refusal surfaces in the neighbouring service rather than in CloudFormation where the defect is.
Fix: the CloudFormation resource provider for AWS::EC2::Instance — it must record the EC2 instance id as the PhysicalResourceId.
Traces: cloudformation__ec2__ssm.cli-script-cfn-update-stack-security-group-blocks-ssm-negative.

## Evidence
Cause 1: `KeyError: 'OutputValue'` raised while reading DescribeStacks output.
Cause 2: `AssertionError: DescribeAutoScalingGroups('sim2real-xsvc-rpa-...-AutoScalingGroup-...') -> 0 groups` / `auto scaling group 'sim2real-xsvc-rpa-fa4231b98548-AutoScalingGroup-BB37432287A4' not found` (the group name is a per-run value).
Cause 3: `AssertionError: expected exactly the overlapping subnet to fail; CREATE_FAILED events were [('Overlap', 'Resource handler returned message: "The CIDR 10.72.0.0/25 conflicts with another subnet (subnet-00000000000000029)" (RequestToken: ..., HandlerErrorCode: ResourceConflict)'), ('sim2real-xsvc-ec2ovl-48367cd7a285', 'The following resource(s) failed to create: [Overlap]. ')]` — the second tuple is the stack itself.
Cause 4: `botocore.errorfactory.InvalidInstanceId: An error occurred (InvalidInstanceId) when calling the DescribeInstanceInformation operation: Value at 'instanceId' failed to satisfy constraint: Member must satisfy regular expression pattern: (^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)`; the sibling symptom in the same family is `An error occurred (InvalidInstanceID.Malformed) when calling the DescribeInstances operation: Invalid id: "sim2real-xsvc-dec3-sg-...-Target-..."`. The CIDRs, stack names and ids above are per-run values; the requirement is that PhysicalResourceId is the `i-...` id.

## Steps to Reproduce
1. Boot the emulator; point cloudformation, ec2, autoscaling and ssm clients at it.
2. Cause 1: CreateStack for any template with an Outputs section, wait for CREATE_COMPLETE, then DescribeStacks and read Stacks[0].Outputs[0]['OutputValue'].
3. Cause 2: CreateStack for a template containing AWS::EC2::LaunchTemplate + AWS::AutoScaling::AutoScalingGroup, wait for CREATE_COMPLETE, read the group's physical id from DescribeStackResources, then DescribeAutoScalingGroups with that name.
4. Cause 3: CreateStack for a template with two AWS::EC2::Subnet resources whose CIDRs overlap; after the failure, DescribeStackEvents and filter ResourceStatus=='CREATE_FAILED'.
5. Cause 4: CreateStack for a template with an AWS::EC2::Instance, read its PhysicalResourceId from DescribeStackResources, then pass it to EC2 DescribeInstances or SSM DescribeInstanceInformation.
6. Re-replay the check ids in trace_ids.
