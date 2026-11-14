Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__elbv2.deploy-cfn-negative-missing-service-target-group-binding
Issue-Key: ecr__ecs__elbv2.deploy-cfn-negative-missing-service-target-group-binding|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: 84939cc283904302bdfe02403fef158918b6c804

# ecr__ecs__elbv2: the CFN negative control for a missing service/target-group binding never reaches a terminal state, so deploy-cfn-negative-missing-service-target-group-binding times out with no verdict

> _Note: this issue was identified against the code base at commit `84939cc283904302bdfe02403fef158918b6c804`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the negative-control stack — an ECR-image-backed ECS service deployed without the loadBalancers binding to the stack's target group — never reaches the terminal state the check needs, so the contrast it exists to prove (no binding => no registered target) is never observable.

AWS: CreateStack settles to a terminal StackStatus, and from then on the estate is readable: ecs DescribeServices/ListTasks report the service's task, and elbv2 DescribeTargetHealth on the stack's target group returns an EMPTY TargetHealthDescriptions list because the service declares no loadBalancers block — the task is never registered. Nothing about the missing binding is an error; the recorded behaviour is a successful stack plus an empty target-health list (contrasted against the positive deploy where the task does register and reaches 'healthy'). Smithy: com.amazonaws.ecs#Deployment.rolloutState -> DeploymentRolloutState enum COMPLETED|FAILED|IN_PROGRESS; the empty-registration outcome is recorded behaviour only.

Emulator: the check reports only `Error: Check timed out after 60.0s`. It is cut off inside the stack-status / service-convergence waits, so neither the stack's terminal status nor the (expected empty) DescribeTargetHealth result is ever obtained — no status code, error code or body is produced.

Why: because the negative control cannot complete, an emulator that registers targets it should not register — or that fails to register the ones it should — is indistinguishable from correct behaviour. A client cannot tell whether omitting loadBalancers has any effect.

Fix: the CloudFormation handler for AWS::ECS::Service / AWS::ElasticLoadBalancingV2::TargetGroup so the stack reaches a terminal status, plus the ECS service scheduler/task state and the ELBv2 target-registration state that DescribeTargetHealth reads.

Related open issues that look similar but do NOT carry this check's replay key: 20261111T095650Z-ecr-ecs-elbv2-ecr-image-backed-ecs-services.md, 20261112T060648Z-ecr-ecs-elbv2-an-ecr-image-backed-fargate.md, 20261110T143849Z-ecr-ecs-elbv2-an-ecs-service-created-with.md.

Traces: the failing check below.

## Evidence
Expected (AWS / recorded): CreateStack reaches a terminal StackStatus; the ECS service exists and places its task; elbv2 DescribeTargetHealth on the stack's target group returns TargetHealthDescriptions == [] because the service declares no loadBalancers binding.

Actual (emulator): `Error: Check timed out after 60.0s` — no terminal stack status, no service convergence, and the DescribeTargetHealth comparison is never performed.

Failing check:
- ecr__ecs__elbv2.deploy-cfn-negative-missing-service-target-group-binding

## Steps to Reproduce
1. Push/declare an ECR-image-backed task definition and CreateStack with an ALB + target group + listener and an AWS::ECS::Service that deliberately OMITS the loadBalancers binding.
2. Poll DescribeStacks for a terminal StackStatus and the ECS service for convergence.
3. Neither settles; the check is killed at the harness's 60 s cap before asserting that DescribeTargetHealth on the target group is empty.
