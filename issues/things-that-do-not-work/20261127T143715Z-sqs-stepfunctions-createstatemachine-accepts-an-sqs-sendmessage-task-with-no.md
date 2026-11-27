Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs__stepfunctions.send-message-task-missing-message-body-rejected
Issue-Key: sqs__stepfunctions.send-message-task-missing-message-body-rejected|_refusal#18|AssertionError|raise AssertionError("a sendMessage Task with no MessageBody was accepted at create")
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# CreateStateMachine accepts an sqs:sendMessage Task with no MessageBody parameter

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStateMachine

## Description
One cause.

Cause 1 of 1: a state machine whose `arn:aws:states:::sqs:sendMessage` Task omits the required `MessageBody` parameter is accepted at create time.

AWS: Step Functions validates a service-integration Task's Parameters against the target API's shape when the definition is submitted. `MessageBody` is required on SQS SendMessage (Smithy: com.amazonaws.sqs SendMessageRequest.MessageBody is required), so the definition is refused with `InvalidDefinition` (Smithy: com.amazonaws.sfn CreateStateMachine error InvalidDefinition, HTTP 400) and the message `Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: The field 'MessageBody' is required but was missing at <state path>'`; no state machine exists afterwards, so DescribeStateMachine on its ARN is refused. The fixed text is the `SCHEMA_VALIDATION_FAILED: The field '…' is required but was missing at …` frame; the field name and the `/States/<State>/Parameters` path come from the submitted definition.

Emulator: the create is accepted, so the invalid definition is published.

Why: the client's omission is not caught where AWS catches it — at deploy — and instead surfaces as a failed execution later, after the state machine has been released; a deploy pipeline that relies on CreateStateMachine to validate its definitions passes a machine AWS would reject.

Fix: the CreateStateMachine/UpdateStateMachine definition validator's service-integration Parameters schema check (required-member validation against the target operation's input shape).

The same missing validator is reported for an unsupported field name under the sns__sqs__stepfunctions heading; an issue carries one service token, so the two headings are filed separately.

Traces: sqs__stepfunctions.send-message-task-missing-message-body-rejected.

## Evidence
Expected: ('InvalidDefinition', "Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: The field 'MessageBody' is required but was missing at /States/Send/Parameters'") and no state machine left behind.
Actual: `AssertionError: a sendMessage Task with no MessageBody was accepted at create`.

## Steps to Reproduce
1. Call CreateStateMachine with a definition containing a single Task of Resource `arn:aws:states:::sqs:sendMessage` whose Parameters set only `QueueUrl` (no `MessageBody`).
2. The call succeeds instead of being refused with InvalidDefinition.
Replay: sqs__stepfunctions.send-message-task-missing-message-body-rejected.
