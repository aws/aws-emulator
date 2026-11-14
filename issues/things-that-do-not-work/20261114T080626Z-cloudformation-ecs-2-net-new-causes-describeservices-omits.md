Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs.cfn-ecs-fargate-service-in-place-update-retained-replacement-then-inactive cloudformation__ecs.deploy-ecs-service-round-trip-then-stack-delete
Issue-Key: cloudformation__ecs.cfn-ecs-fargate-service-in-place-update-retained-replacement-then-inactive|_poll#24|TimeoutError|raise TimeoutError(f"run budget ended while waiting for {what}; last {value!r}")
Issue-Key: cloudformation__ecs.deploy-ecs-service-round-trip-then-stack-delete|run#44|AssertionError|assert len(after) == 1, f"DescribeServices({service_arn!r}) after delete returned {after!r}"
Issue-Commit: dbbfc43f271727daf6a36bf70721e9eb6030bb1c

# cloudformation__ecs: 2 net-new causes — DescribeServices omits the deployment circuit breaker's resetOnHealthyTask/thresholdConfiguration, and a deleted service vanishes from DescribeServices instead of reading back INACTIVE

> _Note: this issue was identified against the code base at commit `dbbfc43f271727daf6a36bf70721e9eb6030bb1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Two independent root causes in the ECS service read-back path, both reached through CloudFormation-provisioned services. Neither check is owned by an open issue; the closest open items (20261111T111216Z-cloudformation-ecs-2-net-new-causes-aws-ecs-cluster-in.md and 20261109T144605Z-cloudformation-ecs-5-net-new-causes-ecs-capacity.md) carry different replay keys and different causes.

Cause 1 of 2: `deploymentConfiguration.deploymentCircuitBreaker` is served with only the two members the template set.
AWS:      Smithy (ecs): DeploymentCircuitBreaker has four members — `enable` (required, default false), `rollback` (required, default false), `resetOnHealthyTask` (boolean, documented "The default is true") and `thresholdConfiguration` (ThresholdConfiguration{type, value}). A service created with DeploymentConfiguration.DeploymentCircuitBreaker{Enable, Rollback} therefore reads back through DescribeServices with the two declared members PLUS the service-applied defaults: resetOnHealthyTask true and thresholdConfiguration {type: BOUNDED_PERCENT, value: 50}.
Emulator: DescribeServices returns `deploymentCircuitBreaker` == {'enable': False, 'rollback': False} — resetOnHealthyTask and thresholdConfiguration are absent, so the check's poll for the service "as templated" never converges and ends at its budget.
Why:      a client reading a service's deployment configuration back (drift detection, a reconciler that compares live state against the template, an operator inspecting circuit-breaker behavior) sees a configuration AWS never serves. Code that reads `resetOnHealthyTask` or the threshold to decide how many failures are tolerated gets a KeyError or silently assumes a different policy, and any round-trip comparison of DescribeServices output against the declared configuration fails.
Fix:      ecs's service state and DescribeServices serializer: CreateService/UpdateService must materialize the DeploymentCircuitBreaker defaults (resetOnHealthyTask true, thresholdConfiguration BOUNDED_PERCENT/50) and DescribeServices must serialize all four members.
Traces:   cloudformation__ecs.cfn-ecs-fargate-service-in-place-update-retained-replacement-then-inactive.

Cause 2 of 2: a service deleted with its stack is no longer describable at all.
AWS:      Smithy (ecs): DescribeServices returns `services` plus a `failures` list. ECS documents no not-found state for a service it just deleted — DeleteService on a service with no tasks to drain moves it to the terminal INACTIVE and the ARN remains readable, so DescribeServices answers with exactly one row in `services` (status INACTIVE) and an empty `failures`.
Emulator: after the stack delete, DescribeServices(cluster=..., services=[<service arn>]) answers with an empty `services` list — the check fails on `len(after) == 1`. The service record is hard-deleted rather than tombstoned.
Why:      a client cannot confirm a clean teardown or distinguish "deleted" from "never existed"; post-delete status assertions and drain monitors break on the empty list.
Fix:      ecs's DeleteService/DescribeServices handlers and service state — retain a terminal INACTIVE record with zero desired/running counts and return it from DescribeServices.
Traces:   cloudformation__ecs.deploy-ecs-service-round-trip-then-stack-delete.

## Evidence
Cause 1 — cfn-ecs-fargate-service-in-place-update-retained-replacement-then-inactive:
  Expected `deployment` view: (maximumPercent, minimumHealthyPercent, {'resetOnHealthyTask': True, 'thresholdConfiguration': {'type': 'BOUNDED_PERCENT', 'value': 50}, 'enable': <declared>, 'rollback': <declared>})
  Actual (last polled value before the budget ended):
    TimeoutError: run budget ended while waiting for service <name> as templated; last {... 'deployment': (100, 0, {'enable': False, 'rollback': False}), ...}
  i.e. deploymentCircuitBreaker carries only enable and rollback.

Cause 2 — deploy-ecs-service-round-trip-then-stack-delete:
  Expected: DescribeServices(cluster=..., services=[<service arn>]) after the stack delete returns one service row (status INACTIVE).
  Actual:
    File "check_cloudformation__ecs_deploy_ecs_service_round_trip_then_stack_delete.py", line 264, in run
    assert len(after) == 1, ...
    AssertionError: DescribeServices('arn:aws:ecs:<region>:<account>:service/<cluster>/<service>') after delete returned []

(Service/cluster names, ARNs and the percentages the template declared are per-run or template-supplied values; the requirements are the presence of the four circuit-breaker members with AWS's defaults, and the INACTIVE row after delete.)

## Steps to Reproduce
Cause 1:
1. CreateStack with an AWS::ECS::Service (LaunchType FARGATE, DesiredCount 0, awsvpc NetworkConfiguration) whose DeploymentConfiguration sets MaximumPercent, MinimumHealthyPercent and DeploymentCircuitBreaker{Enable, Rollback}.
2. Wait for CREATE_COMPLETE.
3. Call ecs DescribeServices(cluster=<cluster arn>, services=[<service arn>], include=["TAGS"]).
4. Observe deploymentConfiguration.deploymentCircuitBreaker == {enable, rollback} with no resetOnHealthyTask and no thresholdConfiguration.

Cause 2:
1. CreateStack with an ECS cluster, Fargate task definition and service; wait for CREATE_COMPLETE and record the service ARN.
2. DeleteStack and wait for DELETE_COMPLETE.
3. Call ecs DescribeServices(cluster=<cluster arn>, services=[<service arn>]).
4. Observe an empty `services` list instead of one INACTIVE row.
