Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.deploy-ecs-task-role-trust-policy-cfn
Issue-Key: autoscaling__ecs__elbv2.deploy-ecs-task-role-trust-policy-cfn|_poll_until#14|Timeout|last = probe()
Issue-Commit: 0e9afbb76f97c880d990543e0ccf2d5bfecf65a1

# autoscaling__ecs__elbv2: a CloudFormation-deployed Fargate service never places a task that reaches RUNNING and ships awslogs output, so the execution-role trust path produces no observable result

> _Note: this issue was identified against the code base at commit `0e9afbb76f97c880d990543e0ccf2d5bfecf65a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
One root cause, grounded in 1 failing check the suite report lists under `### autoscaling__ecs__elbv2`, which is marked neither "ALREADY FILED" nor "OWNED by" any open issue.

Cause 1 of 1: a stack that declares an IAM execution role trusting `ecs-tasks.amazonaws.com`, a log group, an ECS cluster, a FARGATE/awsvpc task definition with an `awslogs` log configuration and an `AWS::ECS::Service` with `DesiredCount: 1` never yields a placed task that reaches RUNNING and never yields container output in the declared log group; the whole lifecycle produces no transition inside the suite's budget.

  AWS: recorded behavior for this template — `CreateStack` reaches CREATE_COMPLETE and exposes the cluster, service and log group as outputs; `ListTasks(cluster, serviceName)` then returns at least one task ARN; `DescribeTasks` reports that task's `lastStatus` reaching `RUNNING` (not `STOPPED`) because the execution role's trust policy lets ECS assume it; and `FilterLogEvents(logGroupName=<declared group>, logStreamNamePrefix="app")` returns at least one event, because the same execution role is what authorises the `awslogs` driver's `logs:CreateLogStream`/`logs:PutLogEvents`. Smithy (com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113): `ListTasks` input {cluster, containerInstance, family, nextToken, maxResults, startedBy, serviceName, desiredStatus, launchType} -> {taskArns, nextToken}; `DescribeTasks` input {cluster, tasks (required), include} -> {tasks, failures}, where `Task.lastStatus`, `stopCode` and `stoppedReason` carry the verdict. The model states the shapes, not the timing; the timing is recorded behavior only — on AWS every one of these steps lands well inside the check's own 400s/180s/120s allowances.

  Emulator: nothing in that chain is observed. The check is cut by the harness: `Error: Check timed out after 60.0s`, inside the CFN deploy / `ListTasks` / `DescribeTasks` / `FilterLogEvents` polling sequence. No modeled error and no successful transition is produced — the stack's ECS service never surfaces a task that reaches a launch verdict, so a broken execution-role trust and a working one are indistinguishable.

  Why: a client cannot tell whether its execution role is correct. The signal AWS gives (task RUNNING + log events present, versus task STOPPED with a `stoppedReason` naming the role) is replaced by permanent silence, so a stack whose IAM trust is wrong looks exactly like one that is right, and nothing that waits on the service's first task ever returns.

  Fix: the ECS service scheduler and task lifecycle behind the CloudFormation-provisioned `AWS::ECS::Service` — task placement and the RUNNING transition for a FARGATE/awsvpc service task, and the `awslogs` log-driver delivery into the declared log group (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_service.rs, .../src/ops/list_tasks.rs, .../src/ops/describe_tasks.rs and the task records in .../src/state.rs, plus whatever ships container output to CloudWatch Logs).

  Traces: autoscaling__ecs__elbv2.deploy-ecs-task-role-trust-policy-cfn.

## Evidence
Expected (recorded AWS): deploy the template (SecurityGroup, IAM ExecutionRole trusting `ecs-tasks.amazonaws.com` with `AmazonECSTaskExecutionRolePolicy`, Logs::LogGroup, ECS::Cluster, FARGATE/awsvpc ECS::TaskDefinition with `awslogs` options `{awslogs-group: !Ref LogGroup, awslogs-region: !Ref AWS::Region, awslogs-stream-prefix: app}`, ECS::Service DesiredCount 1); then
  - `aws ecs list-tasks --cluster <ClusterOut> --service-name <ServiceOut>` -> at least one `taskArns` entry;
  - `aws ecs describe-tasks --cluster <ClusterOut> --tasks <arn>` -> `lastStatus: "RUNNING"` (a `STOPPED` here is the signal that the execution-role trust is broken);
  - `aws logs filter-log-events --log-group-name <LogGroupOut> --log-stream-name-prefix app` -> at least one event.

Actual (emulator): `Error: Check timed out after 60.0s`. The body never gets past its polling loops (`service places a task`, `task reaches a terminal launch state`, `execution role ships container output to CloudWatch Logs`), so neither a RUNNING task nor a STOPPED task with a stoppedReason, nor any log event, is ever produced.

(The VPC, subnet, cluster, service and log-group names are per-run generated values and are not part of the requirement; the requirement is that the service's task reaches a launch verdict and that the awslogs driver delivers at least one event into the group the task definition names.)

## Steps to Reproduce
1. `aws cloudformation create-stack` with the template above, substituting the account's VPC id and one subnet id; wait for CREATE_COMPLETE and read the `ClusterOut`, `ServiceOut`, `LogGroupOut` outputs.
2. Poll `aws ecs list-tasks --cluster <ClusterOut> --service-name <ServiceOut>` until a task ARN appears. On the emulator this never returns one inside the budget.
3. Poll `aws ecs describe-tasks --cluster <ClusterOut> --tasks <arn>` until `lastStatus` is RUNNING or STOPPED; assert RUNNING.
4. Poll `aws logs filter-log-events --log-group-name <LogGroupOut> --log-stream-name-prefix app` until at least one event is returned.
