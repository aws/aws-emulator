Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.ecs-task-state-change-event-delivered-to-eventbridge-rule
Issue-Key: autoscaling__ecs__elbv2.ecs-task-state-change-event-delivered-to-eventbridge-rule|_collect_task_events#38|Timeout|return seen
Issue-Commit: bdca4f331e3f84a5ee858acb1e0644b3d6ffabda

# autoscaling__ecs__elbv2: a Fargate task's RUNNING/STOPPED transitions are never published as 'ECS Task State Change' events, so an EventBridge rule's SQS target stays empty (1 cause)

> _Note: this issue was identified against the code base at commit `bdca4f331e3f84a5ee858acb1e0644b3d6ffabda`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: the ECS task state machine never puts an `ECS Task State Change` envelope on the EventBridge default bus, so a rule that matches that source/detail-type delivers nothing to its target for the whole life of a task.

AWS: a task placed by RunTask advances PENDING -> RUNNING -> STOPPED, and ECS publishes one event per transition onto the default event bus with `source` = `aws.ecs`, `detail-type` = `ECS Task State Change`, and `detail` carrying the Task (Smithy: ECS `Task` with `taskArn`, `clusterArn`, `taskDefinitionArn`, `lastStatus`, `desiredStatus`, `containers`, and `version`, whose doc states "Every time a task experiences a change that starts a CloudWatch event, the version counter is incremented... you can compare the version of a task reported by the Amazon ECS API actions with the version reported in CloudWatch Events for the task (inside the `detail` object)"). An enabled rule whose pattern selects {source: [aws.ecs], detail-type: ['ECS Task State Change'], detail.clusterArn: [<cluster arn>]} with an SQS target (and a queue policy allowing events.amazonaws.com sqs:SendMessage) receives the RUNNING envelope and then the STOPPED envelope, in that chronological order; the STOPPED envelope's `detail.desiredStatus` is `STOPPED`, and DescribeTasks then reports `lastStatus` `STOPPED` in agreement with the delivered event. The fixed text of the envelope is the `source`/`detail-type` pair and the `lastStatus`/`desiredStatus` values; the cluster ARN, task ARN, task-definition ARN, timestamps and version numbers are values taken from the request and from the service's own state, not literals.

Emulator: no message ever arrives on the rule's SQS target. The check's collection loop polls ReceiveMessage until the suite budget is exhausted and the check ends with `Error: Check timed out after 60.0s`, so neither the RUNNING nor the STOPPED envelope is ever observable, and the ordering/`desiredStatus`/version assertions are never even reachable.

Why: a client that drives automation off the task lifecycle through EventBridge — failure alarms, deployment gating, post-stop cleanup, fan-out by cluster — sees an emulator in which tasks apparently never change state. Any consumer that blocks until the STOPPED event arrives hangs forever instead of completing, which is precisely the observed failure mode.

Fix: the ECS task lifecycle's event-emission component — whatever transitions a task's `lastStatus`/`desiredStatus` must serialize the Task (with the incremented `version`) into an `ECS Task State Change` envelope and publish it to the default event bus so the existing EventBridge rule-matching/target-delivery path can route it to the SQS target.

Related already-open issues describe the same silence but do not carry this failure's current replay key, which the suite reports unowned: 20261120T154211Z-autoscaling-ecs-elbv2-ecs-task-state-change-events-are-never-delivered--r2.md (same check, key pinned at `_collect_task_events#27`), 20261104T062603Z-ecs-events-sqs-ecs-never-publishes-deployment-or.md, 20261104T060239Z-cloudwatch-ecr-ecs-fargate-tasks-never-leave-running.md.

Traces: autoscaling__ecs__elbv2.ecs-task-state-change-event-delivered-to-eventbridge-rule.

## Evidence
Expected (AWS): two messages on the rule's SQS target, in order — `{"source":"aws.ecs","detail-type":"ECS Task State Change","detail":{"taskArn":<the task's arn>,"clusterArn":<the cluster's arn>,"lastStatus":"RUNNING",...}}` and then the same envelope with `"lastStatus":"STOPPED"`, `"desiredStatus":"STOPPED"` and a strictly higher `detail.version`; the RUNNING envelope's `time` is <= the STOPPED envelope's `time`; DescribeTasks subsequently reports `lastStatus` = `STOPPED`.

Actual (emulator): zero messages are ever delivered to the queue. The check reports `Error: Check timed out after 60.0s` while still waiting for the first envelope. Fixed text of the requirement: the `source` value `aws.ecs`, the `detail-type` value `ECS Task State Change`, and the `lastStatus`/`desiredStatus` enum values; variable parts: cluster ARN, task ARN, task-definition ARN, `time`, and `version`.

## Steps to Reproduce
1. CreateCluster.
2. RegisterTaskDefinition: networkMode awsvpc, requiresCompatibilities [FARGATE], cpu 256, memory 512, one essential container with a short-lived command.
3. CreateQueue; GetQueueAttributes for QueueArn; SetQueueAttributes with a policy allowing events.amazonaws.com sqs:SendMessage on that queue conditioned on the rule ARN.
4. PutRule with EventPattern {"source":["aws.ecs"],"detail-type":["ECS Task State Change"],"detail":{"clusterArn":[<cluster arn>]}}, State ENABLED; PutTargets pointing Id "1" at the queue ARN.
5. RunTask on the cluster with launchType FARGATE, count 1, awsvpcConfiguration on a public subnet.
6. Poll ReceiveMessage on the queue for the task's RUNNING and then STOPPED `ECS Task State Change` envelopes. No message ever arrives; the check exhausts its budget.
