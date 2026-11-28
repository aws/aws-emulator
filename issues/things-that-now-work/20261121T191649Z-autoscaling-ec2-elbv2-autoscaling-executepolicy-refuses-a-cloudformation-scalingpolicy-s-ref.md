Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.deploy-cfn-asg-scaling-policy-with-target-group
Issue-Key: auto-scaling|ExecutePolicy|ValidationError
Issue-Commit: d9773982178bffc206ea9531196a8841a020d5e3

# autoscaling: ExecutePolicy refuses a CloudFormation ScalingPolicy's Ref value with ValidationError "The AutoScaling Group name is required when specifying a policy by its name" instead of executing the policy named by ARN

> _Note: this issue was identified against the code base at commit `d9773982178bffc206ea9531196a8841a020d5e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecutePolicy

## Description
REOPENS 20261110T143849Z-auto-scaling-executepolicy-stub.md: its fix left the call refused.

Cause 1 of 1: ExecutePolicy, called with the value a CloudFormation `AWS::AutoScaling::ScalingPolicy` resource's `Ref` yields and with no `AutoScalingGroupName`, is refused as if the value were a bare policy NAME.

AWS: Smithy (autoscaling 2011-01-01) `ExecutePolicyType$PolicyName` (ResourceName, len 1..1600) is **required** and documented as "The name or ARN of the policy"; `ExecutePolicyType$AutoScalingGroupName` (XmlStringMaxLen255) is OPTIONAL — an ARN carries the group inside it, so a request that names the policy by ARN needs no group member and succeeds. The operation models only `ScalingActivityInProgressFault` (HTTP 400) and `ResourceContentionFault` (HTTP 500); a `ValidationError` for a well-formed ARN is not among them. CloudFormation's `Ref` on an `AWS::AutoScaling::ScalingPolicy` resolves to that policy's ARN (`arn:aws:autoscaling:<region>:<account>:scalingPolicy:<id>:autoScalingGroupName/<group>:policyName/<policy>`) — that is why the recorded flow exports it as `ScalingPolicyArn` and feeds it straight to `ExecutePolicy(PolicyName=..., HonorCooldown=false)`. On AWS that call answers HTTP 200 with an empty body and applies the policy's `ChangeInCapacity +1` adjustment, raising the group's `DesiredCapacity` and launching a second instance. Recorded behavior only: the subsequent convergence (the new instance reaching `InService` and registering healthy in the wired target group).

Emulator: the same call is refused with HTTP 400, error code `ValidationError`, message `The AutoScaling Group name is required when specifying a policy by its name`. That message is entirely FIXED text — it has no variable part, neither the policy value asked for nor the group that was omitted appears in it (it is emitted verbatim from `GROUP_REQUIRED_FOR_POLICY_NAME_MESSAGE`). It is the sentence AWS uses only when `PolicyName` really is a bare NAME, so the emulator did not recognize the CloudFormation-supplied value as a scaling-policy ARN: either the `AWS::AutoScaling::ScalingPolicy` provisioner's `Ref` resolves to the policy's NAME (or some other non-ARN identifier) rather than its ARN, or the ARN it mints is not accepted by the ARN test the handler gates on (`names_policy_by_arn`, which requires `is_resource_arn_of(value, "autoscaling")` AND `resource_type(value) == "scalingPolicy"`). Hypothesis: the `Ref` value is the policy name, since the handler's ARN branch exists and is reached before any lookup. Whichever of the two it is, the request is rejected before the policy is resolved and nothing of the group is touched — no capacity change, no second instance, no target registration.

Why: a client that deploys the standard template-driven load-balanced auto-scaled service and then forces the scale-out the deterministic way (`aws autoscaling execute-policy --policy-name <the stack's ScalingPolicy Ref> --no-honor-cooldown`, or the SDK equivalent) gets a 400 `ValidationError` its SDK models no error for, instead of a 200 and a `+1` capacity change. The ARN spelling of `PolicyName` — the only identifier CloudFormation hands back for a `ScalingPolicy` — becomes unusable, so stack-declared scaling policies cannot be exercised at all and every downstream assertion (new instance `InService`, registered in the target group, target `healthy`, listener forwarding to it) is unreachable. Passing the group name as a workaround is not available to a client that only holds the stack output.

Fix: the ExecutePolicy handler and the identifier it resolves — `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/execute_policy.rs` (the pre-lookup `group_required_for_policy_name` gate) together with `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/state/policy_execution.rs` (`names_policy_by_arn`) and the scaling-policy ARN minted in `src/state/scaling_policies.rs`, and on the other side the CloudFormation provisioner for `AWS::AutoScaling::ScalingPolicy`, whose `Ref` must resolve to the policy ARN. This is the identifier-resolution path, not the response serializer.

Related open issue describing the same end-to-end flow but carrying a different replay key (the suite did not mark this check owned): 20261108T115606Z-autoscaling-ec2-elbv2-a-cloudformation-deployed-asg-scalingpolicy-target-group-stack.md — that one records the flow timing out while ExecutePolicy was still a 501 scaffold; the stack now converges and the baseline instance reaches `InService`, and the flow stops at this refusal instead.

Traces: autoscaling__ec2__elbv2.deploy-cfn-asg-scaling-policy-with-target-group.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (490/647 passed; the emulator is up and answering — it returned a real modelled refusal here).

[error] autoscaling__ec2__elbv2.deploy-cfn-asg-scaling-policy-with-target-group:
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the ExecutePolicy operation: The AutoScaling Group name is required when specifying a policy by its name

The check reached this call having already passed the earlier legs: the stack (VPC + two subnets, SG with tcp/80 ingress, LaunchTemplate, ElasticLoadBalancingV2 TargetGroup/LoadBalancer/Listener, AutoScalingGroup Min 1 / Max 3 / Desired 1 with TargetGroupARNs, and AWS::AutoScaling::ScalingPolicy SimpleScaling / ChangeInCapacity / ScalingAdjustment 1 / Cooldown '0') reached CREATE_COMPLETE and exported `TargetGroupArn`, `AutoScalingGroupName` and `ScalingPolicyArn`, and exactly one baseline instance reached LifecycleState `InService`. It then calls:

    autoscaling.execute_policy(PolicyName=result["outputs"]["ScalingPolicyArn"], HonorCooldown=False)

Expected (recording): HTTP 200, empty body; the group's DesiredCapacity moves to 2 and a second instance reaches `InService`; DescribeTargetHealth(TargetGroupArn=<tg>) then lists that new instance id with TargetHealth.State `healthy`.

Actual: HTTP 400, error code `ValidationError`, message `The AutoScaling Group name is required when specifying a policy by its name`. Fixed text: the whole sentence (the emulator's constant interpolates nothing). Values taken from the request/state: none — which is itself the tell, because AWS emits this sentence only for a bare policy name, and the request carried the stack's ScalingPolicy `Ref` value with no `AutoScalingGroupName`.

Source of the refusal: `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/execute_policy.rs:197-201` returns `Scaffolded::validation_error(GROUP_REQUIRED_FOR_POLICY_NAME_MESSAGE)` whenever `group_required_for_policy_name(None, &policy_name)` holds, and that predicate (`src/state/policy_execution.rs:284-289`) holds exactly when `names_policy_by_arn(value)` is false — i.e. when the value is not an `autoscaling` resource ARN whose resource type is `scalingPolicy`.

(Stack, group, policy and instance names, ids, the region/account inside any ARN, and timestamps are per-run values; the requirements are that `PolicyName` accepts the ARN spelling with no `AutoScalingGroupName`, that CloudFormation's `Ref` for `AWS::AutoScaling::ScalingPolicy` is that ARN, and that the call answers 200 and applies the +1 adjustment.)

## Steps to Reproduce
1. Boot the emulator and resolve an AMI id (ssm get-parameter /aws/service/ami-amazon-linux-latest/amzn2-ami-hvm-x86_64-gp2).
2. CreateStack with a template containing: AWS::EC2::VPC (10.40.0.0/16) + two Subnets in different AZs, AWS::EC2::SecurityGroup with tcp/80 ingress from 10.40.0.0/16, AWS::EC2::LaunchTemplate (that ImageId, t3.micro, that SG), AWS::ElasticLoadBalancingV2::TargetGroup (VpcId, Port 80, Protocol HTTP, TargetType instance), an internal AWS::ElasticLoadBalancingV2::LoadBalancer + Listener forwarding to it, AWS::AutoScaling::AutoScalingGroup (MinSize '1', MaxSize '3', DesiredCapacity '1', VPCZoneIdentifier the two subnets, TargetGroupARNs [!Ref TargetGroup], DependsOn Listener), and AWS::AutoScaling::ScalingPolicy (AutoScalingGroupName !Ref AutoScalingGroup, PolicyType SimpleScaling, AdjustmentType ChangeInCapacity, ScalingAdjustment 1, Cooldown '0'). Outputs: TargetGroupArn, AutoScalingGroupName, and ScalingPolicyArn = !Ref ScaleOutPolicy.
3. Poll DescribeStacks until terminal; expect CREATE_COMPLETE with all three Outputs. Note the value of the ScalingPolicyArn output — on AWS it is the policy's ARN (arn:aws:autoscaling:<region>:<account>:scalingPolicy:<id>:autoScalingGroupName/<group>:policyName/<policy>).
4. Poll autoscaling DescribeAutoScalingGroups(AutoScalingGroupNames=[<asg>]) until one instance has LifecycleState 'InService'.
5. Call autoscaling ExecutePolicy(PolicyName=<that ScalingPolicyArn output>, HonorCooldown=false) with NO AutoScalingGroupName. Observe HTTP 400 ValidationError 'The AutoScaling Group name is required when specifying a policy by its name' instead of HTTP 200 with an empty body.
6. Also call ExecutePolicy with an ARN obtained from autoscaling DescribePolicies(AutoScalingGroupName=<asg>)["ScalingPolicies"][0]["PolicyARN"] and no AutoScalingGroupName, to separate 'Ref returns a non-ARN' from 'the ARN form is not recognized'.
7. Poll DescribeAutoScalingGroups for a second InService instance and elbv2 DescribeTargetHealth(TargetGroupArn=<tg>) for that instance becoming 'healthy'. Observe neither happens, because the scale-out was never applied.
8. Re-replay autoscaling__ec2__elbv2.deploy-cfn-asg-scaling-policy-with-target-group to confirm a fix.
