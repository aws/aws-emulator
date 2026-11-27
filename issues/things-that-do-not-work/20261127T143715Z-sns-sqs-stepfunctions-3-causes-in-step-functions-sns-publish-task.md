Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__sqs__stepfunctions.sns-publish-task-camel-case-parameter-rejected-while-pascal-case-delivers sns__sqs__stepfunctions.sns-publish-task-serializes-object-message sns__sqs__stepfunctions.sns-publish-wait-for-task-token-resumes-with-the-callback-output
Issue-Key: sns__sqs__stepfunctions.sns-publish-task-camel-case-parameter-rejected-while-pascal-case-delivers|_refusal#21|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: sns__sqs__stepfunctions.sns-publish-task-serializes-object-message|run#14|AssertionError|assert resp["status"] == "SUCCEEDED", (
Issue-Key: sns__sqs__stepfunctions.sns-publish-wait-for-task-token-resumes-with-the-callback-output|run#23|Timeout|time.sleep(min(2 ** (attempt + 1), 15))
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# 3 causes in Step Functions' SNS Publish task: no Parameters schema validation, object Message not serialized, waitForTaskToken never resumes

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStateMachine

## Description
Three causes.

Cause 1 of 3: CreateStateMachine accepts an `arn:aws:states:::sns:publish` Task whose Parameters use a camelCase field name.
AWS: Step Functions validates a service-integration Task's Parameters against the target API's shape at create time. An unsupported field is refused with `InvalidDefinition` (Smithy: com.amazonaws.sfn CreateStateMachine error InvalidDefinition, HTTP 400) and the message `Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: The field "<name>" is not supported by Step Functions. Did you mean '<Correct>'? at <state path>'`, and no state machine is created. The fixed text is the `SCHEMA_VALIDATION_FAILED: The field "…" is not supported by Step Functions. Did you mean '…'? at …` frame; the field name, the suggestion and the `/States/<State>/Parameters` path come from the submitted definition.
Emulator: the call succeeds with HTTP 200 and returns a stateMachineArn, so the definition is accepted and the machine is left behind.
Why: the client's typo is not caught where AWS catches it — at deploy — and surfaces instead as a runtime failure or a silently unset parameter, after the state machine has been published.
Fix: the CreateStateMachine/UpdateStateMachine definition validator's service-integration Parameters schema check.
Traces: sns__sqs__stepfunctions.sns-publish-task-camel-case-parameter-rejected-while-pascal-case-delivers.

Cause 2 of 3: an `sns:publish` Task whose `Message` parameter is a structure fails at runtime instead of publishing the serialized object.
AWS: when the Task's `Message` parameter is a JSON object, Step Functions serializes it to a JSON string and publishes it; the execution succeeds and the subscriber receives that JSON text as the notification's Message (Smithy: com.amazonaws.sns Publish.Message targets the `message` string shape, so the object is serialized into it).
Emulator: the execution ends FAILED with error `States.Runtime` and cause `the request this task would have sent could not be built: this task's 'Message' is a structure, which this emulator does not bind to that service's form-field protocol`.
Why: a very common pattern — passing a structured payload straight into sns:publish — fails every execution, and the failure cause is an emulator-internal explanation rather than any AWS error.
Fix: the Step Functions service-integration request builder's parameter binding for non-string members of an awsQuery/form-encoded target.
Traces: sns__sqs__stepfunctions.sns-publish-task-serializes-object-message.

Cause 3 of 3: an `sns:publish.waitForTaskToken` Task never resumes from the callback.
AWS: the Task publishes the notification carrying the task token, pauses, and resumes with the callback's output when SendTaskSuccess is called with that token; the execution then succeeds with that output.
Emulator: the check never observes the resumption it grades; it ends at its 60 s budget.
Why: the callback pattern is how a client hands work to an out-of-band consumer; an execution that never resumes hangs until its timeout, and the consumer's answer is discarded.
Fix: the Step Functions `.waitForTaskToken` integration for sns:publish (token issuance into the published message, and the SendTaskSuccess resume path).
Traces: sns__sqs__stepfunctions.sns-publish-wait-for-task-token-resumes-with-the-callback-output.

## Evidence
Cause 1 — expected ('InvalidDefinition', 'Invalid State Machine Definition: \'SCHEMA_VALIDATION_FAILED: The field "topicArn" is not supported by Step Functions. Did you mean \'TopicArn\'? at /States/Notify/Parameters\''); actual `expected a refusal, but the call succeeded: {'stateMachineArn': 'arn:aws:states:...:stateMachine:<name>', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`.
Cause 2 — expected the execution to end SUCCEEDED with the object published as JSON text; actual `got 'FAILED' error='States.Runtime' cause="the request this task would have sent could not be built: this task's 'Message' is a structure, which this emulator does not bind to that service's form-field protocol"`.
Cause 3 — expected the execution to resume with the callback output; actual `Check timed out after 60.0s`.

## Steps to Reproduce
Cause 1: CreateStateMachine with a Task `arn:aws:states:::sns:publish` whose Parameters use `topicArn` instead of `TopicArn`.
Cause 2: CreateStateMachine with an sns:publish Task whose `Message` Parameter is a JSON object, and start an execution.
Cause 3: CreateStateMachine with `arn:aws:states:::sns:publish.waitForTaskToken`, start an execution, read the token from the delivered notification and call SendTaskSuccess.
Replay: the three check ids in trace_ids.
