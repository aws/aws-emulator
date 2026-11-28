Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.sqs-esm-mismatched-queue-arn-not-triggered-negative
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-mismatched-queue-arn-not-triggered-negative|run#11|Timeout|resp = sqs.receive_message(QueueUrl=results["url"], MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 23dd5744c97c6d8665be751ea9e1c6568241dbfe

# lambda__sqs__stepfunctions: an Enabled SQS event source mapping never delivers for its own wired queue, so the mismatched-queue negative scenario reaches no verdict

> _Note: this issue was identified against the code base at commit `23dd5744c97c6d8665be751ea9e1c6568241dbfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: an Enabled SQS event source mapping created against one queue never invokes its target function, so the scenario that must show the mapping NOT firing for a message put on an unrelated queue can never be graded — its positive control (the same mapping DOES deliver for the queue it is wired to) never lands and the check spends its whole 60 s budget without reaching an assertion. Failing check: lambda__sqs__stepfunctions.sqs-esm-mismatched-queue-arn-not-triggered-negative.

AWS: Smithy (lambda) models CreateEventSourceMapping as POST /2015-03-31/event-source-mappings -> 202 with EventSourceMappingConfiguration (UUID, EventSourceArn, State, BatchSize 1..10000, FunctionArn, LastProcessingResult); modeled errors InvalidParameterValueException 400, ResourceConflictException 409, ResourceNotFoundException 404, TooManyRequestsException 429, ServiceException 500. A mapping polls exactly the queue named by EventSourceArn: once it reads back State=Enabled, a SendMessage to THAT queue is read by the Lambda poller, delivered to the function as an SQS `Records` batch (at most BatchSize records) and removed from the queue on a successful handler return, while a message sent to any other queue is never read, never produces an invocation, and stays in its own queue (SQS GetQueueAttributes ApproximateNumberOfMessages stays 1). Recorded behavior only for the delivery latency: AWS delivers within seconds of the mapping becoming Enabled.

Emulator: CreateEventSourceMapping is accepted and the mapping reads back Enabled through GetEventSourceMapping, but no invocation ever happens for either queue. The function never writes its echo to the results queue for the wired queue either, so the delivery wait never satisfies and the check ends with "Error: Check timed out after 60.0s" (the 60 s figure is the harness budget, not a requirement). Hypothesis: the emulator keeps the event source mapping purely as control-plane state and runs no SQS poller, so "correctly does not fire for an unrelated queue" is indistinguishable from "never fires at all".

Why: a client wiring an SQS queue to a function through an event source mapping gets a healthy-looking Enabled mapping that never consumes anything — queued work is never processed, batches are never delivered, and a test suite relying on the mapping to isolate one queue from another cannot tell a correctly scoped mapping from a dead one.

Fix: the Lambda event-source-mapping poller for SQS sources (the ESM runtime/state behind CreateEventSourceMapping / GetEventSourceMapping), which must, for an Enabled mapping, poll only the queue named by EventSourceArn, invoke the function with the read messages as an SQS Records batch, delete the successfully handled messages, and leave messages on every other queue untouched.

Related open issues 20261125T165255Z-lambda-sqs-the-sqs-event-source-mapping-filtercriteria.md, 20261029T072224Z-lambda-s3-sqs-an-sqs-event-source-mapping.md and 20261122T004908Z-dynamodb-lambda-sqs-the-sqs-queue-policy-deny.md describe the same missing SQS delivery in other service groups; none of them carries this check's replay key, so this check is still ungraded.

Traces: lambda__sqs__stepfunctions.sqs-esm-mismatched-queue-arn-not-triggered-negative.

## Evidence
Expected (AWS): with an Enabled SQS event source mapping (EventSourceArn = wired queue, BatchSize 10) on a python3.12 function, (a) a SendMessage to an UNRELATED queue produces no invocation and that message remains in its queue (GetQueueAttributes ApproximateNumberOfMessages = 1), and (b) a SendMessage to the WIRED queue is delivered within seconds as an SQS Records batch, the handler echoes the record body, and the body read back equals the body sent.

Actual (emulator): CreateEventSourceMapping returns 202 and GetEventSourceMapping reports State=Enabled, but no invocation is ever observed for either queue — the positive-control message put on the wired queue is never delivered to the function, so nothing is ever written to the results queue and the check terminates with "Error: Check timed out after 60.0s" before any AWS assertion is evaluated. No error code or non-2xx is returned by any control-plane call; the divergence is the absent data-plane delivery.

## Steps to Reproduce
1. Create three SQS queues: an unwired queue, a wired queue and a results queue.
2. Create an IAM role trusted by lambda.amazonaws.com with AWSLambdaSQSQueueExecutionRole and AmazonSQSFullAccess attached.
3. CreateFunction (Runtime=python3.12, Handler=index.handler, Publish=true) whose handler forwards every event["Records"][i]["body"] to the results queue; wait for Configuration.State=Active.
4. CreateEventSourceMapping with EventSourceArn=<wired queue arn>, FunctionName=<fn>, BatchSize=10; poll GetEventSourceMapping until State is Enabled.
5. SendMessage to the UNWIRED queue, then poll the results queue for up to 60 s and confirm nothing arrives, and that GetQueueAttributes on the unwired queue still reports ApproximateNumberOfMessages=1.
6. SendMessage "should-be-consumed" to the WIRED queue and poll the results queue.
7. Observe the wired-queue message is never delivered either: the results queue stays empty and the check is killed at its 60 s budget with no verdict.
