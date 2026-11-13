Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs.deploy-cfn-ecr-repository-policy-restricts-cross-account-pull-rejected
Issue-Key: ecs|DescribeTaskDefinition|ClientException
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# A CloudFormation stack declaring AWS::ECR::Repository + AWS::ECS::TaskDefinition reaches CREATE_COMPLETE but registers nothing in ECS

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation does not provision the AWS::ECS::TaskDefinition (nor the AWS::ECR::Repository it DependsOn) that the deployed stack declares, yet the stack completes successfully.
AWS: recorded behavior only (CloudFormation resource provisioning is not described by the ECS model). When CreateStack for a template containing AWS::ECR::Repository and an AWS::ECS::TaskDefinition that references the repository's image URI reaches CREATE_COMPLETE, ecs DescribeTaskDefinition(taskDefinition=<Family>) returns the registered revision, and its containerDefinitions[0].image is the repository URI the template built.
Emulator: the stack deploy returns as complete, and the very next read fails — DescribeTaskDefinition answers ClientException (HTTP 400) "Unable to describe task definition." — so the task definition was never registered. The check never reaches the cross-account-pull assertions it exists to grade.
Why: a client that deploys its service through CloudFormation is told the deployment succeeded while ECS holds no task definition; every downstream step (RunTask, service create, image-URI verification) then fails for an unrelated-looking reason, and there is no FAILED status or rollback to point at the real cause.
Fix: the CloudFormation resource provisioner must materialise AWS::ECS::TaskDefinition (and the AWS::ECR::Repository it depends on) into the respective service state, and must fail the stack rather than report CREATE_COMPLETE when it cannot. Similar gaps are already filed for other resource types (e.g. 20261022T171541Z-codedeploy-..., 20261022T210606Z-sagemaker-...); neither covers these resource types.
Traces: cloudwatch__ecr__ecs.deploy-cfn-ecr-repository-policy-restricts-cross-account-pull-rejected.

## Evidence
After _deploy_cfn(...) returned for a template whose Resources are {Repository: AWS::ECR::Repository, TaskDefinition: AWS::ECS::TaskDefinition (DependsOn Repository)}:
expected: describe_task_definition(taskDefinition=<family>) -> taskDefinition with taskDefinitionArn and containerDefinitions[0].image == <account>.dkr.ecr.<region>.<domain>/<repo>:latest
actual: botocore.errorfactory.ClientException: An error occurred (ClientException) when calling the DescribeTaskDefinition operation: Unable to describe task definition.

## Steps to Reproduce
1. cloudformation CreateStack with a template declaring an AWS::ECR::Repository and an AWS::ECS::TaskDefinition (Family=F, one container whose image is the repository URI, DependsOn the repository). 2. Wait for the stack to report CREATE_COMPLETE. 3. Call ecs DescribeTaskDefinition(taskDefinition=F) -> observe ClientException instead of the registered revision.
