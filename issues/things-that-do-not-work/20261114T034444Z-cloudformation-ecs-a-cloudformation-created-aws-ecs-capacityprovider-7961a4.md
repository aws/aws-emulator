Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs.deploy-ecs-capacity-provider-round-trip-then-delete cloudformation__ecs.deploy-ecs-capacity-provider-with-nonexistent-asg-rejected
Issue-Key: cloudformation__ecs.deploy-ecs-capacity-provider-round-trip-then-delete|run#14|AssertionError|assert listed == [cp_name], (
Issue-Key: cloudformation__ecs.deploy-ecs-capacity-provider-with-nonexistent-asg-rejected|run#12|AssertionError|assert control == [("ACTIVE", f"autoScalingGroupName/{good_asg}", "DISABLED")], (
Issue-Commit: 1d457c6418d8d9996f08d1176214615fe9686e31

# cloudformation__ecs: a CloudFormation-created AWS::ECS::CapacityProvider is invisible to ECS — DescribeCapacityProviders lists nothing for it

> _Note: this issue was identified against the code base at commit `1d457c6418d8d9996f08d1176214615fe9686e31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCapacityProviders

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: a stack that reaches CREATE_COMPLETE with an AWS::ECS::CapacityProvider resource leaves no capacity provider in ECS — an unfiltered, fully paged DescribeCapacityProviders returns an empty capacityProviders list for the provider the stack declared.
AWS: Smithy (ecs) DescribeCapacityProviders output {capacityProviders: CapacityProviders, failures: Failures, nextToken: String}; called with no capacityProviders filter it pages every provider in the account/Region. On AWS, a CREATE_COMPLETE stack's AWS::ECS::CapacityProvider is a real ECS capacity provider: the paged listing contains exactly that provider name, with status ACTIVE, autoScalingGroupProvider.autoScalingGroupArn naming the declared ASG (the check's control read expects (status ACTIVE, autoScalingGroupName/<asg>, managedScaling DISABLED)) (recorded behavior only — the model does not describe the CFN resource's effect).
Emulator: after the stack reports its resources created, the paged DescribeCapacityProviders answers an empty list ([]), so the declared provider does not exist in ECS at all; the control-provider read in the negative check likewise reads [].
Why: a client that provisions ECS capacity declaratively gets a stack that claims success over a resource that was never created. Every later read or use of that provider (DescribeCapacityProviders, PutClusterCapacityProviders, a capacityProviderStrategy on RunTask/CreateService) behaves as if the provider does not exist, and a stack delete cannot be verified either.
Fix: the CloudFormation AWS::ECS::CapacityProvider resource handler — it must create the provider in ECS's own capacity-provider state (the state DescribeCapacityProviders reads and deletes from on stack delete), rather than only recording stack-level resource status.
Traces: cloudformation__ecs.deploy-ecs-capacity-provider-round-trip-then-delete, cloudformation__ecs.deploy-ecs-capacity-provider-with-nonexistent-asg-rejected (the latter's valid control provider reads empty, which is what prevents its negative leg from being judged).

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator.

[mismatch] cloudformation__ecs.deploy-ecs-capacity-provider-round-trip-then-delete
  File "check_cloudformation__ecs_deploy_ecs_capacity_provider_round_trip_then_delete.py", line 224: assert listed == [cp_name]
  AssertionError: an unfiltered, fully paged DescribeCapacityProviders lists [] starting with 'sim2real-cfncp-cp-<suffix>', expected exactly ['sim2real-cfncp-cp-<suffix>']
  (the name is a per-run generated value; the requirement is that the provider the stack declared is listed, not that literal name)

[mismatch] cloudformation__ecs.deploy-ecs-capacity-provider-with-nonexistent-asg-rejected
  File "check_cloudformation__ecs_deploy_ecs_capacity_provider_with_nonexistent_asg_rejected.py", line 237: assert control == [("ACTIVE", f"autoScalingGroupName/{good_asg}", "DISABLED")]
  AssertionError: control capacity provider reads []

Expected (AWS): DescribeCapacityProviders (no filter, fully paged) contains the stack's provider with status ACTIVE and autoScalingGroupProvider naming the declared ASG. Actual (emulator): empty capacityProviders list.

Smithy consulted (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/smithy-model/com-amazonaws-ecs-amazonec2containerservicev20141113.json): DescribeCapacityProviders output {capacityProviders, failures, nextToken}, readonly.

## Steps to Reproduce
1. Clone the repo and replay suite simparity-behavioral-aws-v1; both check ids in trace_ids reproduce.
2. Create an Auto Scaling group, then CreateStack with a template declaring an AWS::ECS::CapacityProvider whose AutoScalingGroupProvider.AutoScalingGroupArn is that ASG and ManagedScaling Status DISABLED; wait for CREATE_COMPLETE.
3. Call DescribeCapacityProviders with no capacityProviders filter and page with nextToken.
4. Expected: the listing contains the declared provider (status ACTIVE, autoScalingGroupProvider naming the ASG). Actual: capacityProviders is empty.
5. DeleteStack and confirm the provider is then gone from the same listing.
6. Re-replay both ids in trace_ids to confirm the fix.
