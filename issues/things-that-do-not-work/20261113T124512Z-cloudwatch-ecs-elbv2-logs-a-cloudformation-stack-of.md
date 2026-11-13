Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.deploy-stack-without-log-permission-no-log-group-created
Issue-Key: cloudwatch__ecs__elbv2__logs.deploy-stack-without-log-permission-no-log-group-created|_poll_until#22|Timeout|last = probe()
Issue-Commit: 6ec43c73f71e6013077e783d53ce3ba5d179d34d

# cloudwatch__ecs__elbv2__logs: a CloudFormation stack of ECS + ELBv2 + Logs resources never reaches a terminal status, exhausting the check's budget

> _Note: this issue was identified against the code base at commit `6ec43c73f71e6013077e783d53ce3ba5d179d34d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the permission-boundary scenario cannot get past its stack deploy — the whole check is killed at its 60s wall-clock budget while waiting on the CloudFormation stack it created (the deploy helper itself allows 300s), so no ECS/awslogs assertion is ever reached.
AWS: recorded behavior only. CreateStack for this template (cluster, two task definitions, two log groups, subnets/SG outputs) settles to CREATE_COMPLETE promptly and DescribeStacks reports a terminal StackStatus with its Outputs populated, so the recorded run proceeds to RunTask; the subsequent assertions are about a task whose execution role lacks logs:CreateLogGroup never reaching RUNNING and never creating its group.
Emulator: the check ends with 'Check timed out after 60.0s' — the stack never reaches a terminal status (or does so only after the budget), so the outputs the scenario needs (ClusterArn, SubnetIds, DefaultSgId, TaskDefBadArn/TaskDefGoodArn, LogGroupBadName/LogGroupGoodName) are never available and nothing downstream is exercised.
Why: a client's deploy-then-exercise loop hangs: a stack that neither completes nor fails is the worst outcome of the three, because there is no status to branch on and no rollback to recover from. It also masks the awslogs permission behaviour entirely.
Fix: the CloudFormation engine's create path for this resource mix (stack status progression to a terminal state and Outputs resolution) in the cloudformation crate, and whichever of the ECS/Logs/ELBv2 resource providers it blocks on.
Traces: cloudwatch__ecs__elbv2__logs.deploy-stack-without-log-permission-no-log-group-created.

## Evidence
Error: Check timed out after 60.0s, with the check's first action being `_deploy_cfn(ctx, template, timeout_s=300.0)` followed by reads of outputs ClusterArn/SubnetIds/DefaultSgId/TaskDefBadArn/TaskDefGoodArn/LogGroupBadName/LogGroupGoodName. Expected: DescribeStacks reaches a terminal CREATE_COMPLETE (or a rollback) with those Outputs populated.

## Steps to Reproduce
CreateStack with a template declaring an AWS::ECS::Cluster, two AWS::ECS::TaskDefinition resources using the awslogs driver with awslogs-create-group (one execution role with logs:CreateLogGroup, one without), two AWS::Logs::LogGroup resources, and VPC/subnet/security-group resources with Outputs; poll DescribeStacks for a terminal status and read its Outputs.
