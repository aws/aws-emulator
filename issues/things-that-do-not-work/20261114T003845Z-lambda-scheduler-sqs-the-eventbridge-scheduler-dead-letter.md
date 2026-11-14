Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-deleted-lambda-target-dead-letters-once
Issue-Key: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-deleted-lambda-target-dead-letters-once|run#39|AssertionError|assert body.get("InvocationType") == "Event", (
Issue-Commit: 2ac91d382b67c33b3b1ad8ceed805222f48956bf

# lambda__scheduler__sqs: the EventBridge Scheduler dead-letter body for a Lambda universal target omits InvocationType, so the dead-lettered request does not record the asynchronous invoke

> _Note: this issue was identified against the code base at commit `2ac91d382b67c33b3b1ad8ceed805222f48956bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (EventBridge Scheduler universal target, dead-letter payload)

## Description
Cause 1 of 1: when Scheduler dead-letters a Lambda universal-target invocation, the SQS message body is missing the InvocationType member of the Lambda Invoke request it attempted.
AWS: an EventBridge Scheduler schedule whose target is the `arn:aws:scheduler:::aws-sdk:lambda:invoke` universal target invokes Lambda ASYNCHRONOUSLY, and when the target cannot be invoked (here a deleted function, a non-retryable ResourceNotFound) the schedule's DeadLetterConfig queue receives one message whose message attributes carry ERROR_CODE/ERROR_MESSAGE/RETRY_ATTEMPTS/TARGET_ARN/SCHEDULE_ARN and whose BODY is the Lambda Invoke request document Scheduler built: `{"FunctionName": "<target ARN>", "InvocationType": "Event", "Payload": "<the schedule's Input>"}`. The member names are the Lambda API's; `InvocationType` is `Event` because Scheduler's Lambda target is asynchronous. Documented in the EventBridge Scheduler user guide ('Universal targets' and 'Handling schedule failures'); recorded behavior as far as the Smithy models go.
Emulator: the dead-letter body carries FunctionName and Payload but `InvocationType` is absent (`body.get("InvocationType")` is None), so the recorded request does not state that the invoke was asynchronous.
Why: the dead-letter body is the replay artifact — a client retries a failed schedule by submitting that document to Lambda. Without InvocationType the replay defaults to a synchronous RequestResponse invoke, which changes the semantics of the retry (blocking call, different error surface, different concurrency accounting) compared to what Scheduler originally attempted.
Fix: the EventBridge Scheduler universal-target dead-letter writer — serialize the full Lambda Invoke request document, including InvocationType "Event", into the DLQ message body.
Traces: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-deleted-lambda-target-dead-letters-once.

## Evidence
lambda__scheduler__sqs.xsvc-lambda-3-scheduler-deleted-lambda-target-dead-letters-once — AssertionError: Scheduler must invoke a Lambda target asynchronously; body says None.
Expected per AWS: the single DLQ message's Body is {"FunctionName": "<deleted function ARN>", "InvocationType": "Event", "Payload": "<the schedule's Input>"}; the emulator's body omits the InvocationType member (FunctionName and Payload were accepted by the preceding assertions).

## Steps to Reproduce
1. Create a Lambda function and an SQS dead-letter queue; create a Scheduler schedule whose target is arn:aws:scheduler:::aws-sdk:lambda:invoke for that function, with an Input, a RetryPolicy and a DeadLetterConfig naming the queue.
2. Delete the function so the invoke fails non-retryably, and wait for the schedule's nominal time.
3. Receive the dead-letter message and parse its Body: AWS includes "InvocationType": "Event"; the emulator omits it.
