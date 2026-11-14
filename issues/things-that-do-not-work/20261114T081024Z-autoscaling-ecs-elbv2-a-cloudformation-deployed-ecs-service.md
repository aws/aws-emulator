Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.deploy-ecs-task-role-trust-policy-cfn autoscaling__ecs__elbv2.deploy-ecs-service-missing-target-group-permission-fails-stack
Issue-Key: autoscaling__ecs__elbv2.deploy-ecs-task-role-trust-policy-cfn|_poll_until#14|Timeout|last = probe()
Issue-Key: autoscaling__ecs__elbv2.deploy-ecs-service-missing-target-group-permission-fails-stack|_poll_until#15|Timeout|last = probe()
Issue-Commit: 84939cc283904302bdfe02403fef158918b6c804

# autoscaling__ecs__elbv2: a CloudFormation-deployed ECS service never places a task that reaches RUNNING (and ships no awslogs output), so both execution-role/target-group deploy checks time out with no verdict

> _Note: this issue was identified against the code base at commit `84939cc283904302bdfe02403fef158918b6c804`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
Cause 1 of 1: a CloudFormation-deployed ECS estate (task execution role trusting ecs-tasks.amazonaws.com, awslogs log group, ALB target group) never yields the signals the checks poll for — the service never places a task that reaches a terminal launch state, so neither the positive execution-role path nor the missing-target-group-permission negative control reaches an assertion.

AWS: after CreateStack reaches a terminal StackStatus, ecs ListTasks(cluster, serviceName) returns at least one task ARN, and DescribeTasks reports that task's lastStatus reaching a terminal launch state — RUNNING when the execution role's trust policy and permissions are correct, STOPPED otherwise. With a correct execution role the container's stdout is then delivered to the declared CloudWatch Logs group, so logs FilterLogEvents(logGroupName, logStreamNamePrefix='app') returns at least one event. For the negative template (no elasticloadbalancing permission for the service) the stack instead ends in a failed/rolled-back terminal status and the task is never registered as a target. Smithy: com.amazonaws.ecs#Deployment.rolloutState -> DeploymentRolloutState enum COMPLETED|FAILED|IN_PROGRESS; the lastStatus progression itself is recorded behaviour only.

Emulator: no terminal signal appears. Both checks report only `Check timed out after 60.0s`, cut off inside their first wait (the stack poll with a 300-400 s budget, then the 180 s ListTasks poll, then the 180 s DescribeTasks lastStatus poll, then the 120 s FilterLogEvents poll). No status, error code or message is produced at all.

Why: a client that deploys an ECS service by CloudFormation and waits for its task to run — the standard way to verify an execution role or an IAM permission boundary — never gets an answer. Both the success case (role trusted, task RUNNING, logs delivered) and the refusal case (missing elasticloadbalancing permission must fail the stack) are invisible, so neither can be depended on.

Fix: the ECS service scheduler/task state machine behind ListTasks and DescribeTasks.lastStatus, the awslogs log-driver delivery into CloudWatch Logs, and the CloudFormation handler for AWS::ECS::Service / AWS::IAM::Role so a stack reaches a terminal status.

Related open issues that look similar but do NOT carry these checks' replay keys: 20261111T093414Z-autoscaling-ecs-elbv2-cloudformation-stacks-combining-an-auto.md, 20261104T042502Z-autoscaling-ecs-elbv2-6-net-new-root-causes.md, 20261114T034444Z-autoscaling-ecs-elbv2-2-net-new-causes-a.md.

Traces: the two failing checks below, both with the identical timeout detail.

## Evidence
Expected (AWS / recorded): ListTasks returns >= 1 task ARN; DescribeTasks reports lastStatus == 'RUNNING' for the correctly-trusted execution role; FilterLogEvents on the stack's log group with stream prefix 'app' returns >= 1 event; for the negative template the stack ends in a failed terminal status and no target is registered.

Actual (emulator): `Error: Check timed out after 60.0s` for both checks — no task ARN, no lastStatus, no log event, and no stack terminal status is ever observed. The checks' own budgets (stack 400 s, ListTasks 180 s, lastStatus 180 s, logs 120 s) all exceed the harness cap, so the failure is a stalled wait rather than a wrong response.

Failing checks:
- autoscaling__ecs__elbv2.deploy-ecs-task-role-trust-policy-cfn
- autoscaling__ecs__elbv2.deploy-ecs-service-missing-target-group-permission-fails-stack

## Steps to Reproduce
1. CreateStack with an AWS::IAM::Role whose trust policy allows ecs-tasks.amazonaws.com, an AWS::Logs::LogGroup, an AWS::ECS::Cluster, an AWS::ECS::TaskDefinition using the awslogs driver into that group, and an AWS::ECS::Service with desiredCount 1 in the default VPC's subnet.
2. Poll DescribeStacks until StackStatus is terminal.
3. Poll ecs ListTasks(cluster=<ClusterOut>, serviceName=<ServiceOut>) for >= 1 task ARN -> never satisfied.
4. Poll ecs DescribeTasks for lastStatus in {RUNNING, STOPPED} -> never satisfied.
5. Repeat with the negative template whose service role lacks elasticloadbalancing permission and expect the stack to fail terminally -> no terminal status either.
6. Both runs are killed by the harness at 60 s.
