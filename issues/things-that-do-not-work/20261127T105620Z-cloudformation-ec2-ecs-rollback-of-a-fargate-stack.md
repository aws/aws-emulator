Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ecs.deploy-cfn-fargate-service-later-failure-rollback-releases-task-enis
Issue-Key: cloudformation__ec2__ecs.deploy-cfn-fargate-service-later-failure-rollback-releases-task-enis|run#25|AssertionError|assert bad["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# cloudformation__ec2__ecs: rollback of a Fargate stack ends in ROLLBACK_FAILED — the task's ENI is never released, so the security group, subnet and gateway attachment fail to delete with DependencyViolation

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: resources a Fargate task depends on cannot be deleted during rollback because the task ENI is never released, leaving the stack in ROLLBACK_FAILED instead of ROLLBACK_COMPLETE.
AWS: when a later resource in a CreateStack fails (here a subnet whose CIDR conflicts with an existing one, correctly refused with EC2 InvalidSubnet.Conflict), CloudFormation rolls the stack back and deletes every resource it created. Deleting the ECS service/task releases the task's elastic network interface first, so the task subnet, its security group and the VPC gateway attachment then delete cleanly and the stack reaches StackStatus ROLLBACK_COMPLETE with no resource left in DELETE_FAILED (recorded behavior; the DependencyViolation refusals EC2 raises are correct in isolation — what diverges is that the dependency still exists at rollback time).
Emulator: the stack ends in `ROLLBACK_FAILED` with StatusReason `The following resource(s) failed to delete: [<security group>, <task subnet>, <gateway attachment>].` and three resources in DELETE_FAILED, each quoting an EC2 DependencyViolation (HTTP 400): the security group reported as having a dependent object, the task subnet reported as having dependencies and not deletable, and the gateway attachment refused because the VPC still has mapped public addresses. All three are the signature of a Fargate task ENI that was never detached/deleted when its service was rolled back.
Why: a client cannot rely on rollback being clean. The stack is stuck in ROLLBACK_FAILED (needing manual intervention or ContinueUpdateRollback), leaked networking resources block later deploys of the same template, and automation that waits for ROLLBACK_COMPLETE after a failed deploy never returns.
Fix: task/ENI teardown ordering in the rollback-delete path — the ECS service/task deletion must release the task's ENI (and the public-address mapping that goes with assignPublicIp) before dependent EC2 resources are deleted. This sits in the CloudFormation rollback delete sequencing (services/service-com-amazonaws-cloudformation-cloudformation/src/) together with the ECS task-ENI attachment state in the ECS service.
Traces: check cloudformation__ec2__ecs.deploy-cfn-fargate-service-later-failure-rollback-releases-task-enis.

## Evidence
Expected: bad['StackStatus'] == 'ROLLBACK_COMPLETE' with no DELETE_FAILED resources after the deliberate late failure.
Actual: stack ended 'ROLLBACK_FAILED' with reason 'The following resource(s) failed to delete: [Fence, TaskNet, GwLink]. '; resource failures included ('Clash','CREATE_FAILED', EC2 InvalidSubnet.Conflict for the clashing CIDR — the intended trigger), ('Fence','DELETE_FAILED', 'resource sg-... has a dependent object (Service: Ec2; Status Code: 400; Error Code: DependencyViolation ...)'), ('TaskNet','DELETE_FAILED', "The subnet 'subnet-...' has dependencies and cannot be deleted. (... Error Code: DependencyViolation ...)"), ('GwLink','DELETE_FAILED', 'Network vpc-... has some mapped public address(es). Please unmap those public address(es) before detaching the gateway. (... Error Code: DependencyViolation ...)'). Resource ids, request ids and tokens shown are per-run values; the diverging facts are the terminal ROLLBACK_FAILED status and the three DELETE_FAILED dependency refusals.

## Steps to Reproduce
Replay check cloudformation__ec2__ecs.deploy-cfn-fargate-service-later-failure-rollback-releases-task-enis: CreateStack with a VPC, internet gateway + attachment, routes, a task subnet, a security group, an ECS cluster, a task definition and a Fargate service running one task, plus a second subnet whose CidrBlock clashes with the task subnet so it fails after the service was created. Wait for the stack to reach a terminal state and inspect StackStatus and DescribeStackEvents/DescribeStackResources for DELETE_FAILED resources.
