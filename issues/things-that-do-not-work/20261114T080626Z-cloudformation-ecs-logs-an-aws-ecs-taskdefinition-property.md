Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs__logs.deploy-ecs-2-cfn-circuit-breaker-rolls-stack-back-to-last-good-token
Issue-Key: cloudformation__ecs__logs.deploy-ecs-2-cfn-circuit-breaker-rolls-stack-back-to-last-good-token|_wait_stack#22|AssertionError|raise AssertionError(f"stack {stack} settled at {status!r}, wanted {sorted(want)!r}; "
Issue-Commit: dbbfc43f271727daf6a36bf70721e9eb6030bb1c

# cloudformation__ecs__logs: an AWS::ECS::TaskDefinition property update answers NotImplemented and rolls the stack back instead of registering a new revision

> _Note: this issue was identified against the code base at commit `dbbfc43f271727daf6a36bf70721e9eb6030bb1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: updating a task definition's properties in a stack is refused by the resource handler, so the stack reaches UPDATE_ROLLBACK_COMPLETE.
AWS:      recorded behavior only for the CloudFormation resource semantics, grounded in the modeled API: ecs RegisterTaskDefinition always creates a NEW revision of the family — a task definition revision is immutable. AWS::ECS::TaskDefinition therefore has no in-place update at all: any property change is a REPLACEMENT, and CloudFormation registers a new revision, repoints the dependent AWS::ECS::Service at it and the stack reaches UPDATE_COMPLETE. (The check then exercises the ECS deployment circuit breaker rolling forward to that new revision and back.)
Emulator: the TaskDef resource reports UPDATE_FAILED with the reason `NotImplemented: this emulator does not change the properties of an existing AWS::ECS::TaskDefinition in place; the resource was left as it is.` and the stack settles at UPDATE_ROLLBACK_COMPLETE instead of UPDATE_COMPLETE.
Why:      a client cannot ship a new container image, command, environment variable or log configuration through CloudFormation — the ordinary way every containerised service is deployed. The update is rolled back, the service keeps the old revision, and nothing downstream (the circuit-breaker roll-forward, the rollback to the last good revision, the stack-level token the check verifies) can happen. The failure reason is also emulator-authored prose that no AWS client will ever see, so error-handling keyed on AWS's reasons does not match.
Fix:      the AWS::ECS::TaskDefinition resource handler's update path in the CloudFormation provisioner: a property change must be treated as a replacement that calls RegisterTaskDefinition for a new revision (and lets the dependent service be updated to it), not answered with a NotImplemented reason.
Note on an existing issue: the replay report lists this check as unowned. The open issue 20261113T124512Z-cloudformation-ecs-2-net-new-causes-aws-ecs.md names the same emulator refusal under the `cloudformation__ecs` service heading; it does not carry this check's replay key, and this issue is filed under the service heading the suite lists this check under.
Traces:   cloudformation__ecs__logs.deploy-ecs-2-cfn-circuit-breaker-rolls-stack-back-to-last-good-token.

## Evidence
Expected: UpdateStack changing the AWS::ECS::TaskDefinition's container properties reaches UPDATE_COMPLETE, having registered a new revision of the family.
Actual:
  File "check_cloudformation__ecs__logs_deploy_ecs_2_cfn_circuit_breaker_rolls_stack_back_to_last_good_token.py", line 366, in _wait_stack
  AssertionError: stack <name> settled at 'UPDATE_ROLLBACK_COMPLETE', wanted ['UPDATE_COMPLETE']; reasons: [('TaskDef', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::ECS::TaskDefinition in place; the resource was left as it is.')]

The diverging parts are the stack status (UPDATE_ROLLBACK_COMPLETE instead of UPDATE_COMPLETE) and the resource failure reason, whose text is emulator-authored and has no AWS counterpart. The stack name is a per-run value.

## Steps to Reproduce
1. CreateStack with a Logs log group, an ECS cluster, an AWS::ECS::TaskDefinition (Fargate, awslogs log configuration) and an AWS::ECS::Service with DeploymentConfiguration.DeploymentCircuitBreaker{Enable: true, Rollback: true}; wait for CREATE_COMPLETE.
2. UpdateStack with the same template but a changed container property on the task definition (for example a new command/environment token).
3. Poll DescribeStacks until terminal.
4. Observe UPDATE_ROLLBACK_COMPLETE with the TaskDef resource reporting UPDATE_FAILED and the reason "NotImplemented: this emulator does not change the properties of an existing AWS::ECS::TaskDefinition in place; the resource was left as it is."
