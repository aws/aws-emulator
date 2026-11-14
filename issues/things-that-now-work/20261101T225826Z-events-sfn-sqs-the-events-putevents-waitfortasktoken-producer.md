Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs__stepfunctions.xsvc-sfn-2-waittoken-producer
Issue-Key: events__sqs__stepfunctions.xsvc-sfn-2-waittoken-producer|run#20|Timeout|d = sfn.describe_execution(executionArn=exec_arn)
Issue-Commit: 82a9e97b43fb811c245308aabae5044dc3bd73ae

# events__sfn__sqs: the `events:putEvents.waitForTaskToken` producer round trip never completes — no execution of the waitForTaskToken state machine reaches SUCCEEDED (1 cause)

> _Note: this issue was identified against the code base at commit `82a9e97b43fb811c245308aabae5044dc3bd73ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One net-new root cause, grounded in one failing behavioral check.

Cause 1 of 1: a state machine whose single Task is `arn:aws:states:::events:putEvents.waitForTaskToken` never completes its four-hop round trip. Nothing resumes the paused execution: it stays RUNNING until the attempt budget is spent, so DescribeExecution never reports SUCCEEDED and the execution's output never equals the Lambda worker's payload. Failing check: events__sfn__sqs.xsvc-sfn-2-waittoken-producer.

AWS: Smithy (services/service-com-amazonaws-sfn-awsstepfunctions): SendTaskSuccess takes taskToken (required, @length 1..2048, @sensitive) and output (required, @length 0..262144, @sensitive) and models InvalidOutput, InvalidToken, TaskDoesNotExist and TaskTimedOut (all client, HTTP 400); DescribeExecution returns a required `status` from the ExecutionStatus enum plus `output`/`outputDetails`. The `.waitForTaskToken` integration semantics are not in the model, so they are recorded behavior only: on entering the Task, Step Functions substitutes the context object `$$.Task.Token` into the Parameters it renders, calls events:PutEvents once with the rendered Entries (EventBusName, Source, DetailType, Detail — Detail carrying the task token and the per-launch job id), and then PAUSES the execution; it does not advance on the PutEvents response. EventBridge matches the published entry against the bus's rules by exact field equality and invokes each matching target (here an SQS queue and a Lambda function); a sibling rule whose detail-type differs ("task.requested.x" vs "task.requested") matches nothing from that same publication. When the Lambda target calls SendTaskSuccess with the token it read out of the delivered envelope and an output it derived from that envelope, the paused Task resumes and the execution reaches SUCCEEDED with `output` exactly the string passed to SendTaskSuccess. Every value here (token, job id, event id, detail-type, source) comes from the request and the delivered envelope, never a literal from one run.

Emulator: `assert final is not None` — "no execution of the waitForTaskToken machine reached SUCCEEDED within 200s over N attempt(s)" — is never satisfied; the replay's only observable for this check is `Check timed out after 60.0s`, i.e. the check is still relaunching and polling DescribeExecution when the harness bound expires. Every individual call (CreateStateMachine, StartExecution, PutRule, PutTargets, DescribeExecution) answers 2xx, so the divergence is the absent resumption, not a refusal: the chain breaks at one or more of (a) substituting `$$.Task.Token` into the rendered Entries, (b) routing the Task's PutEvents entry through rule matching to the Lambda and SQS targets on the custom bus, (c) pausing rather than completing/failing the Task, or (d) resuming the paused Task from SendTaskSuccess with the supplied output.

Why: the callback pattern — the documented way to drive an asynchronous worker from Step Functions — never completes. A client's execution hangs until its TimeoutSeconds (240 in this check) instead of succeeding with the worker's result, and the output a downstream state would consume is never produced. Because the execution never succeeds, the paired negative guarantee (the sibling rule on detail-type "task.requested.x" receives nothing from the same publish) is also unverifiable.

Fix: the sfn crate's Task-state interpreter for the `.waitForTaskToken` service integration — context-object substitution of `$$.Task.Token` into the rendered Parameters, the pause/park of the Task, and resumption from SendTaskSuccess (setting the execution's `output` to the submitted string and `status` to SUCCEEDED) — together with the events:putEvents integration actually publishing onto the named bus so EventBridge rule matching can invoke the Lambda and SQS targets.

Related but NOT covering this check: the open issues 20261029T182228Z-events-sfn-sqs-no-step-functions-execution-status-change-event-ever.md (which owns the other events__sfn__sqs checks, about execution status-change EVENTS) and 20261023T114649Z-sfn-step-functions-never-executes-task-states-every-execution.md / 20261024T043150Z-sfn-5-of-the-6-interpreter-causes-remain-open.md (Task-state interpretation) are adjacent; none carries this check's replay key, which is why it is filed here.

Traces: events__sfn__sqs.xsvc-sfn-2-waittoken-producer.

## Evidence
Expected (AWS): on a custom event bus carrying a mismatch rule (detail-type "task.requested.x") targeting one SQS queue and a match rule (detail-type "task.requested", detail.nonce pinned) targeting both another SQS queue and a Lambda worker, StartExecution on a state machine whose single Task is arn:aws:states:::events:putEvents.waitForTaskToken (Parameters.Entries[0].Detail = {"taskToken.$": "$$.Task.Token", "job.$": "$.job", "nonce": <nonce>}, TimeoutSeconds 240) leads to: the entry published on that bus, the match rule invoking the worker, the worker calling SendTaskSuccess(taskToken=detail.taskToken, output=<payload derived from the delivered envelope's id, detail-type, source and detail.job>), and DescribeExecution then reporting status "SUCCEEDED" with output exactly that payload — while the mismatch rule's queue receives nothing from the same publish.

Actual (emulator): no execution ever leaves RUNNING; the check's assertion "no execution of the waitForTaskToken machine reached SUCCEEDED within 200s over N attempt(s)" is the one it is heading for, and the suite reports `Error: Check timed out after 60.0s` while it is still relaunching and polling DescribeExecution. All control-plane and StartExecution calls answer 2xx; the task token is never redeemed and the execution is never resumed.

## Steps to Reproduce
1. Create a custom EventBridge event bus and two SQS queues with policies admitting events.amazonaws.com.
2. On that bus create a rule with pattern {"source":["sim.sfn"],"detail-type":["task.requested.x"],"detail":{"nonce":[<nonce>]}} targeting the first queue, then a rule with pattern {"source":["sim.sfn"],"detail-type":["task.requested"],"detail":{"nonce":[<nonce>]}} targeting the second queue.
3. Create a Lambda function that reads event["detail"]["taskToken"] and calls stepfunctions SendTaskSuccess with an output derived from event["id"], event["detail-type"], event["source"] and event["detail"]["job"], grant it states:SendTaskSuccess, and add it as a second target of the matching rule.
4. Create a role trusting states.amazonaws.com with events:PutEvents on the bus ARN, and a state machine with one Task: Resource arn:aws:states:::events:putEvents.waitForTaskToken, Parameters.Entries[0] = {EventBusName: <bus ARN>, Source: "sim.sfn", DetailType: "task.requested", Detail: {"taskToken.$": "$$.Task.Token", "job.$": "$.job", "nonce": <nonce>}}, TimeoutSeconds 240, End true.
5. StartExecution with input {"job": "job-<nonce>-0"} and poll DescribeExecution.
6. Observe the execution never reaches SUCCEEDED (it stays RUNNING): the task token is never delivered/redeemed, so the check relaunches until its budget and the harness bound (60 s) expires.
