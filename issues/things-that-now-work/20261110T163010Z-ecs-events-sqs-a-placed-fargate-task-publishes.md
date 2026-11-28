Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__sqs.a-runtask-that-failed-placement-emits-no-task-state-change-event
Issue-Key: ecs__events__sqs.a-runtask-that-failed-placement-emits-no-task-state-change-event|_poll_until#42|Timeout|time.sleep(interval_s)
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# ecs__events__sqs: a placed Fargate task publishes no 'ECS Task State Change' event to EventBridge, so the failed-placement negative cannot establish its positive control

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: ECS emits no 'ECS Task State Change' event for a task that was really placed, so an EventBridge rule matching source 'aws.ecs' / detail-type 'ECS Task State Change' with a detail.clusterArn filter and an SQS target never receives anything.
AWS: recorded behavior only (the event stream is not modelled in Smithy). Every task state transition is published to the default event bus as an event with source 'aws.ecs', detail-type 'ECS Task State Change' and a detail carrying clusterArn, taskArn and lastStatus; a rule whose pattern pins that source/detail-type and the cluster's ARN delivers those events to its SQS target within seconds of RunTask and of the subsequent StopTask. A RunTask that fails placement creates no task, so no further event naming another taskArn arrives.
Emulator: the positive control never arrives. After RunTask(launchType=FARGATE) is accepted and returns one task, the check drains the target queue for up to 300 s looking for any message whose detail.taskArn equals the placed task's ARN, and the runner kills the check at 60 s with no such message. Because the control is never observed, the check's actual subject — that an EC2-launch-type RunTask on a cluster with zero container instances is refused synchronously with InvalidParameterException 'No Container Instances were found in your cluster.' and emits no event — is never graded.
Why: ECS task state changes are the standard trigger for job-completion handlers, alerting and cleanup. With the stream silent, every ECS-driven EventBridge workflow is dead, and the absence of an unwanted event can never be distinguished from a delivery path that was never live.
Fix: ECS's task-lifecycle event emission onto the EventBridge default bus (source/detail-type/detail envelope for task state changes), and its routing through rule matching to the SQS target.
Traces: ecs__events__sqs.a-runtask-that-failed-placement-emits-no-task-state-change-event. Open issue 20261104T062603Z-ecs-events-sqs-ecs-never-publishes-deployment-or.md describes the same family for other checks in this service but does not carry this one.

## Evidence
ecs__events__sqs.a-runtask-that-failed-placement-emits-no-task-state-change-event — `Error: Check timed out after 60.0s`.
Setup: CreateCluster (0 registered container instances), RegisterTaskDefinition (FARGATE+EC2, awsvpc), an SQS queue with a policy allowing events.amazonaws.com:SendMessage, and PutRule with EventPattern {"source":["aws.ecs"],"detail-type":["ECS Task State Change"],"detail":{"clusterArn":[<cluster arn>]}} plus PutTargets to the queue.
Expected: after RunTask(launchType='FARGATE', count=1) returns one task, at least one queue message whose body has source 'aws.ecs', detail-type 'ECS Task State Change', detail.clusterArn = the cluster ARN and detail.taskArn = the placed task's ARN, with a detail.lastStatus.
Actual: the queue stays empty for the whole run; the control poll (`an 'ECS Task State Change' event for the placed task ... reached <queue url>`) never succeeds and the check is killed at the runner budget before its own negative (InvalidParameterException 'No Container Instances were found in your cluster.' for launchType='EC2' on an empty cluster, and no second task in any desiredStatus) is exercised.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay ecs__events__sqs.a-runtask-that-failed-placement-emits-no-task-state-change-event.
3. Or manually: create an ECS cluster and a FARGATE/awsvpc task definition; create an SQS queue and allow events.amazonaws.com to SendMessage to it; PutRule with pattern {"source":["aws.ecs"],"detail-type":["ECS Task State Change"],"detail":{"clusterArn":[<cluster arn>]}} and PutTargets to that queue; RunTask(count=1, launchType='FARGATE'), then StopTask, and poll ReceiveMessage for events naming the task ARN.
