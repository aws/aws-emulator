Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ecs.cfn-ecs-service-resource-lifecycle
Issue-Key: cloudformation__ec2__ecs.cfn-ecs-service-resource-lifecycle|run#35|AssertionError|assert not redescribed["failures"], (
Issue-Commit: dbbfc43f271727daf6a36bf70721e9eb6030bb1c

# cloudformation__ec2__ecs: after the stack delete, DescribeServices reports the service in `failures` with reason MISSING instead of returning it INACTIVE

> _Note: this issue was identified against the code base at commit `dbbfc43f271727daf6a36bf70721e9eb6030bb1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Cause 1 of 1: a deleted ECS service becomes unknown to DescribeServices instead of readable as INACTIVE.
AWS:      Smithy (ecs): DescribeServices' output carries both `services` and a `failures` list of Failure{arn, reason, detail}; the Failure reason vocabulary (API failure reasons) uses MISSING for a resource that does not exist. ECS documents no not-found state for a service it has just deleted: DeleteService on a service with no tasks to drain moves it straight from ACTIVE to the terminal INACTIVE, and the service remains readable through DescribeServices — the row comes back under `services` with status INACTIVE and desiredCount/runningCount 0, and `failures` is empty. So after CloudFormation deletes the AWS::ECS::Service, the ARN still describes.
Emulator: DescribeServices(cluster=<cluster arn>, services=[<service arn>]) answers with an EMPTY `services` list and `failures` == [{'arn': '<service arn>', 'reason': 'MISSING'}]. The service record is hard-deleted rather than tombstoned as INACTIVE.
Why:      a client that deletes a service and then confirms its terminal state — a teardown assertion, a drain monitor, a reconciler comparing desired against actual — is told the ARN never existed. It cannot distinguish "deleted cleanly" from "never created" or "wrong cluster", and any code that reads the post-delete status (status INACTIVE, counts zero) breaks on an empty list.
Fix:      the ECS service state and the DeleteService/DescribeServices handlers: DeleteService must retain the service as a terminal INACTIVE record with zero desired/running counts, and DescribeServices must return it under `services` rather than reporting MISSING in `failures`.
Note on an existing issue: the open issue 20261109T223435Z-cloudformation-ec2-ecs-an-ecs-service-a-stack.md reads similarly (an ECS service a stack reports CREATE_COMPLETE is reported MISSING by DescribeServices), but the replay report lists this check as unowned — that issue does not carry this check's replay key, and it is about the service being MISSING while the stack is live, not about the post-delete INACTIVE tombstone this check asserts.
Traces:   cloudformation__ec2__ecs.cfn-ecs-service-resource-lifecycle.

## Evidence
Expected, after the stack that owns the AWS::ECS::Service reaches DELETE_COMPLETE:
  DescribeServices(cluster=<cluster arn>, services=[<service arn>]) -> failures == [] and services == [ {status: "INACTIVE", desiredCount: 0, runningCount: 0, ...} ]
Actual:
  File "check_cloudformation__ec2__ecs_cfn_ecs_service_resource_lifecycle.py", line 232, in run
  assert not redescribed["failures"], ...
  AssertionError: DescribeServices reported failures for the deleted service: [{'arn': 'arn:aws:ecs:<region>:<account>:service/<cluster>/<service>', 'reason': 'MISSING'}]

The diverging parts are the shape of the response (a MISSING Failure instead of an INACTIVE service row). The cluster, service and account values in the recorded ARN are per-run.

## Steps to Reproduce
1. CreateStack with a VPC/subnets/security group, an AWS::ECS::Cluster, a Fargate AWS::ECS::TaskDefinition and an AWS::ECS::Service (DesiredCount 0, awsvpc network configuration), plus Outputs for the subnet/SG ids and Fn::GetAtt ServiceArn/Name.
2. Wait for CREATE_COMPLETE and record the service's PhysicalResourceId and the cluster ARN.
3. DeleteStack and wait for DELETE_COMPLETE.
4. Call ecs DescribeServices(cluster=<cluster arn>, services=[<service arn>]).
5. Observe failures == [{arn, reason: 'MISSING'}] and services == [] instead of one INACTIVE service row with zero counts.
