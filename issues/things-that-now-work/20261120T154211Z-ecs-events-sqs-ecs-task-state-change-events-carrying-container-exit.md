Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other
Issue-Key: ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other|_drain_once#25|Timeout|return sink
Issue-Commit: a14965d6060559849ea419b74faa25e7836c4558

# ECS Task State Change events carrying container exit codes are never delivered, so exit-code-pattern EventBridge rules route nothing

> _Note: this issue was identified against the code base at commit `a14965d6060559849ea419b74faa25e7836c4558`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: two EventBridge rules whose patterns select on the container exit code inside an `ECS Task State Change` detail never receive anything, because ECS publishes no task state change event at all.

AWS: when a task stops, ECS publishes an `ECS Task State Change` event (`source` = `aws.ecs`, `detail-type` = `ECS Task State Change`) whose `detail` is the Task shape — Smithy `Task` with `lastStatus` STOPPED, `stopCode`, `stoppedReason`, and `containers[]` each carrying their `exitCode`. An EventBridge rule whose pattern matches `detail.containers.exitCode` therefore routes the zero-exit task to one target and the non-zero-exit task to the other, and each rule's target receives only its own task — the negative half of the differential.

Emulator: no `ECS Task State Change` event is ever published, so both rules stay silent and the check cannot even observe the positive delivery, let alone the non-delivery to the sibling rule. The check ends `Check timed out after 60.0s`.

Why: exit-code routing is the standard way to react to a failed one-shot task (retry, alert, dead-letter). A client sees both rules deliver nothing, so a success path and a failure path are indistinguishable, and any consumer that blocks until its rule fires hangs. A negative-delivery assertion also becomes vacuous: nothing arrives anywhere, which is not the same guarantee as "arrives only at the matching rule".

Fix: the ECS task state machine's event emission — the terminal transition must publish an `ECS Task State Change` envelope whose `detail` includes `containers[].exitCode`, `stopCode` and `stoppedReason`, onto the default bus, so EventBridge pattern matching on `detail.containers.exitCode` can discriminate. Related open issue 20261104T062603Z-ecs-events-sqs-ecs-never-publishes-deployment-or.md describes the same silence for other events in this service but does not carry this check's replay key.

Traces: ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other

## Evidence
Expected: the rule matching a zero exit code receives exactly one `{"source":"aws.ecs","detail-type":"ECS Task State Change","detail":{"lastStatus":"STOPPED","containers":[{"exitCode":0,...}]}}` for the succeeding task and nothing for the failing one; the rule matching the non-zero exit code receives exactly one event with that container `exitCode` and nothing for the succeeding task. Fixed text: the source/detail-type pair and `lastStatus` STOPPED; the exit code value, arns and timestamps are values from the request and emulator state.

Actual: neither rule's SQS target ever receives a message; the check ends `Check timed out after 60.0s`.

## Steps to Reproduce
1. CreateCluster; RegisterTaskDefinition (FARGATE) for a container that exits 0 and one that exits with a non-zero code.
2. Create two SQS queues; PutRule twice with patterns selecting source `aws.ecs`, detail-type `ECS Task State Change` and a specific `detail.containers.exitCode`; PutTargets each at its own queue.
3. RunTask both tasks and wait for them to stop.
4. ReceiveMessage on each queue: the matching event never arrives on either.
