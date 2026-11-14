Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.deploy-ecs-service-missing-target-group-permission-fails-stack autoscaling__ecs__elbv2.ecs-capacity-provider-scales-asg-desired-count
Issue-Key: autoscaling__ecs__elbv2.deploy-ecs-service-missing-target-group-permission-fails-stack|<lambda>#14|IndexError|lambda: ecs.describe_services(
Issue-Key: autoscaling__ecs__elbv2.ecs-capacity-provider-scales-asg-desired-count|_poll_until#46|Timeout|last = probe()
Issue-Commit: 1d457c6418d8d9996f08d1176214615fe9686e31

# autoscaling__ecs__elbv2: 2 net-new causes — a stack-created ECS service is invisible to DescribeServices, and ECS managed scaling never raises the capacity provider's ASG desired capacity

> _Note: this issue was identified against the code base at commit `1d457c6418d8d9996f08d1176214615fe9686e31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Two net-new root causes, both grounded in failing checks of this replay that no open issue carries.

Cause 1 of 2: an ECS service created by a CloudFormation stack is not visible to DescribeServices — the response carries an empty services list.
AWS: Smithy (ecs) DescribeServices output {services: Services, failures: Failures}; a service named in the request that exists is returned in services (and one that does not exist is reported in failures, never silently dropped). On AWS the positive stack's AWS::ECS::Service is a real service, so DescribeServices(cluster, services=[<name>])['services'][0] exists and its runningCount rises to the desired count as its task starts (recorded behavior only for the CFN resource's effect).
Emulator: for the service the stack created, DescribeServices returns services == [] (the check's probe raises IndexError: list index out of range indexing ['services'][0]), even though the container instance of the same cluster had already registered.
Why: a client cannot read back a declaratively provisioned service at all: the read it uses to wait for steady state returns neither the service nor a failure entry, so the service is simply unobservable and no deployment can be verified.
Fix: the CloudFormation AWS::ECS::Service resource handler and the ECS service state it writes — the created service must land in the same state DescribeServices/ListServices read, and DescribeServices must report a named-but-missing service under failures rather than returning an empty services list.
Traces: autoscaling__ecs__elbv2.deploy-ecs-service-missing-target-group-permission-fails-stack.

Cause 2 of 2: ECS managed scaling never raises the desired capacity of a capacity provider's Auto Scaling group, so the scenario never settles.
AWS: recorded behavior only — the scenario creates a capacity provider over a 0-capacity ASG with managed scaling enabled, associates it with a cluster and creates a service with a capacity-provider strategy whose task cannot be placed; on AWS ECS managed scaling then raises the ASG's DesiredCapacity above 0 and an instance registers as a container instance, well inside the check's own internal budgets.
Emulator: the check produces no verdict at all and is killed with "Check timed out after 60.0s" — the ASG's desired capacity never moves and no capacity ever appears.
Why: a client relying on managed scaling for EC2 capacity gets a service whose tasks stay unplaceable forever, with no scaling activity and no error to observe.
Fix: the ECS managed-scaling component for capacity providers (the path that reacts to unplaceable provisioning tasks by calling the Auto Scaling group's desired-capacity update) and the ASG desired-capacity state it must write.
Traces: autoscaling__ecs__elbv2.ecs-capacity-provider-scales-asg-desired-count.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator.

[error] autoscaling__ecs__elbv2.deploy-ecs-service-missing-target-group-permission-fails-stack
  File "check_...deploy_ecs_service_missing_target_group_permission_fails_stack.py", line 324, in _poll_until -> last = probe()
  File "...", line 258, in <lambda> -> lambda: ecs.describe_services(cluster=cluster_name_pos, services=[svc_pos_name])["services"][0]["runningCount"]
  IndexError: list index out of range
  Expected (AWS): DescribeServices returns the stack's service in services (runningCount reaching 1). Actual (emulator): services is empty, with no corresponding failures entry.

[mismatch] autoscaling__ecs__elbv2.ecs-capacity-provider-scales-asg-desired-count
  Error: Check timed out after 60.0s (no verdict; the check waits for ECS managed scaling to raise the 0-capacity ASG's DesiredCapacity for an unplaceable service task)

Smithy consulted (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/smithy-model/com-amazonaws-ecs-amazonec2containerservicev20141113.json): DescribeServices output {services, failures}.

## Steps to Reproduce
1. Clone the repo and replay suite simparity-behavioral-aws-v1; both check ids in trace_ids reproduce.
2. Cause 1: CreateStack with a template containing an ECS cluster, an EC2 container instance, a target group and an AWS::ECS::Service; after the stack's resources are created call DescribeServices(cluster=<cluster>, services=[<service name>]). Expected: services contains the service and runningCount rises to the desired count. Actual: services == [].
3. Cause 2: create an ASG with min/desired 0, CreateCapacityProvider over it with ManagedScaling ENABLED, PutClusterCapacityProviders, then CreateService with that capacity-provider strategy and desiredCount 1.
4. Expected: DescribeAutoScalingGroups shows DesiredCapacity rising above 0 and an instance registering as a container instance. Actual: nothing changes and the check reaches no verdict inside the harness budget.
5. Re-replay both ids in trace_ids to confirm the fix.
