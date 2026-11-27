Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other
Issue-Key: ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other|_drain_once#25|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Commit: bdca4f331e3f84a5ee858acb1e0644b3d6ffabda

# ecs__events__sqs: 'ECS Task State Change' events are never published, so exit-code-scoped EventBridge rules deliver nothing and selective routing cannot be observed (1 cause)

> _Note: this issue was identified against the code base at commit `bdca4f331e3f84a5ee858acb1e0644b3d6ffabda`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: ECS publishes no `ECS Task State Change` events, so rules whose patterns differ only in `detail.containers.exitCode` / `detail.lastStatus` all receive nothing and a client cannot distinguish a correctly-scoped rule from a broken one.

AWS: two Fargate tasks from the same task definition, overridden to exit 7 and exit 0, each advance to STOPPED, and ECS publishes one `ECS Task State Change` event per transition (source `aws.ecs`, detail-type `ECS Task State Change`) onto the default bus. EventBridge then routes each STOPPED envelope only to the rule whose pattern matches that task's `detail.containers[].exitCode`, and both RUNNING envelopes to the rule matching `detail.lastStatus: ["RUNNING"]`. The STOPPED envelope's `detail` agrees with DescribeTasks on the Task shape (Smithy: ECS `Task.stopCode` (enum TaskStopCode), `Task.stoppedReason`, `Task.containers` -> `Container.exitCode`), and `Task.version` is documented as incremented on "a change that starts a CloudWatch event" so every RUNNING envelope's `detail.version` is strictly lower than the STOPPED one's. For an essential container that exits, `stopCode` is `EssentialContainerExited`. The envelope's `resources` list is exactly the one task ARN, and a RUNNING envelope's containers carry no `exitCode` at all. The fixed text here is the `source`/`detail-type` pair, the `lastStatus` values and the `EssentialContainerExited` stop code; the task ARNs, cluster ARN, exit codes, `stoppedReason` text and `version` numbers are values taken from the request and the service's own state.

Emulator: none of the three queues ever receives a message. The check's drain loop polls all three SQS targets until the suite budget is exhausted and the check ends with `Error: Check timed out after 60.0s`, so neither the positive deliveries nor the negative (not-delivered-to-the-other-rule) controls can be evaluated.

Why: an event-driven consumer that reacts to a task's exit status — alerting only on non-zero exits, fanning out stopped tasks by exit code, correlating `version` ordering between RUNNING and STOPPED — gets no events at all, and the negative control is indistinguishable from the positive one, so a client cannot verify that its rule patterns are scoped correctly.

Fix: the ECS task lifecycle's event-emission component must serialize each task transition as an `ECS Task State Change` envelope (source `aws.ecs`, `resources` = [task ARN], `detail` = the Task with incremented `version`, including `containers[].exitCode`, `stopCode` and `stoppedReason` on STOPPED and no `exitCode` on RUNNING) and publish it to the default event bus so the existing EventBridge pattern-matching path can route it per rule.

Related already-open issue 20261104T062603Z-ecs-events-sqs-ecs-never-publishes-deployment-or.md records the same silence; the suite reports this failure as unowned, so its current replay key is not carried there.

Traces: ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other.

## Evidence
Expected (AWS): the rule with `detail.containers.exitCode: [7]` receives exactly the STOPPED envelope of the task that exited 7 and nothing for the other task; the rule with `detail.containers.exitCode: [0]` receives exactly the STOPPED envelope of the task that exited 0; the rule with `detail.lastStatus: ["RUNNING"]` receives both tasks' RUNNING envelopes. Each STOPPED envelope has `resources` = [that task's ARN], `detail.lastStatus` = "STOPPED", `detail.containers[].exitCode` equal to DescribeTasks' value, `detail.stopCode` = "EssentialContainerExited" equal to DescribeTasks', `detail.stoppedReason` equal to DescribeTasks', and a `detail.version` strictly greater than every RUNNING envelope's; RUNNING envelopes carry no `exitCode`.

Actual (emulator): all three queues stay empty for the entire wait; the check reports `Error: Check timed out after 60.0s` before reaching any of the routing or field-agreement assertions.

## Steps to Reproduce
1. CreateCluster; RegisterTaskDefinition (FARGATE, awsvpc, cpu 256, memory 512, one essential container).
2. CreateQueue three times; PutRule + PutTargets three times with patterns scoped to {source:["aws.ecs"], detail-type:["ECS Task State Change"], detail.clusterArn:[<cluster arn>]} plus, respectively, detail {lastStatus:["STOPPED"], containers:{exitCode:[7]}}, detail {lastStatus:["STOPPED"], containers:{exitCode:[0]}}, and detail {lastStatus:["RUNNING"]}.
3. RunTask twice on that cluster with containerOverrides commands `sh -c "sleep 5; exit 7"` and `sh -c "sleep 5; exit 0"`.
4. Poll DescribeTasks until both tasks report lastStatus STOPPED, then poll ReceiveMessage on all three queues.
5. No queue ever receives a message; the check exhausts its budget instead of reaching a routing verdict.
