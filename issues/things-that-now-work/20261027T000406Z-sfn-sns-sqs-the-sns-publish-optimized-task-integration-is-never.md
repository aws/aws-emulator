Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__sqs__stepfunctions.sfn-sns-publish-missing-permission-catch-denied-negative
Issue-Key: sns__sqs__stepfunctions.sfn-sns-publish-missing-permission-catch-denied-negative|run#20|AssertionError|assert good["status"] == "SUCCEEDED", (
Issue-Commit: 6402e991ee1ffb1f37749ece0148ecf3009a760a

# The sns:publish optimized Task integration is never executed: the positive control execution ends FAILED, so a Step Functions Task can never deliver to an SNS topic (1 cause)

> _Note: this issue was identified against the code base at commit `6402e991ee1ffb1f37749ece0148ecf3009a760a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One net-new root cause. The open issue 20261023T114649Z-sfn-step-functions-never-executes-task-states-every-execution.md reports Task states failing in the general case and 20261024T043150Z-sfn-step-functions-interpreter-does-not-run-the-workflow.md covers the interpreter broadly; neither carries this replay key and neither names the `sns:publish` optimized integration or its cross-service delivery, so this is filed separately.

Cause 1 of 1: a Task whose Resource is `arn:aws:states:::sns:publish` does not publish — the execution fails instead of succeeding.
AWS: recorded behavior only (the ASL optimized SNS integration). A Task `{"Type": "Task", "Resource": "arn:aws:states:::sns:publish", "Parameters": {"TopicArn": <topic>, "Message.$": "$.message"}}` run under a role holding `sns:Publish` on that topic calls SNS Publish, records TaskSucceeded with the publish result, and the execution ends `SUCCEEDED`. The message is then delivered to an SQS queue subscribed to the topic. When the role lacks `sns:Publish`, the same Task fails with SNS's AccessDenied/AuthorizationError, the state's `Catch` on `States.ALL` routes the execution to its `Denied` state (so the execution still ends `SUCCEEDED`, via the Catch, not as a direct FAILED), the history carries a TaskFailed event whose cause is SNS's AuthorizationError text, and the subscribed queue receives nothing.
Emulator: the positive control — the permitted leg, with the role's `sns:Publish` grant in place and confirmed via GetRolePolicy before the execution starts — ends `status='FAILED'`. Because the permitted leg already fails, the whole discrimination this check performs collapses: the emulator reports the same FAILED outcome whether or not the role is authorized, so an authorized publish and an unauthorized one are indistinguishable and the state machine's own `Catch`-to-`Denied` routing is never exercised.
Why: no Step Functions workflow can fan a message out through SNS. A caller sees StartExecution return 200 and then an execution that failed for a definition AWS runs; nothing is ever delivered to the subscribed queue. Critically, because the authorized and unauthorized legs produce the identical FAILED verdict, a caller cannot tell a permission problem from a working publish — an emulator that fails both is as wrong for the negative leg as for the positive one, and any client that treats FAILED as "denied" will mis-classify a correctly-permitted call.
Fix: the optimized-integration dispatcher for the `sns:publish` resource in the sfn crate's ASL interpreter — call SNS Publish with the Task's Parameters, emit TaskSucceeded on success (so the execution ends SUCCEEDED and the topic's SQS subscribers receive the message) and exactly one TaskFailed carrying SNS's own AuthorizationError error name and cause when the execution role is not authorized, so the state's Catch can route on it.
Traces: sfn__sns__sqs.sfn-sns-publish-missing-permission-catch-denied-negative.

## Evidence
sfn__sns__sqs.sfn-sns-publish-missing-permission-catch-denied-negative:
  AssertionError: Positive control expected SUCCEEDED, got: 'FAILED'

  Setup at the point of failure (all of it confirmed before the assertion):
    - SNS topic created; SQS queue created, its Policy granting sns.amazonaws.com sqs:SendMessage with aws:SourceArn = the topic; subscription created with Protocol=sqs and polled until PendingConfirmation == "false".
    - Execution role created with the states.amazonaws.com trust policy and an inline policy {"Effect":"Allow","Action":"sns:Publish","Resource":<topic arn>}, polled via GetRolePolicy until readable.
    - STANDARD state machine: {"StartAt":"PublishToSNS","States":{"PublishToSNS":{"Type":"Task","Resource":"arn:aws:states:::sns:publish","Parameters":{"TopicArn":<topic arn>,"Message.$":"$.message"},"Catch":[{"ErrorEquals":["States.ALL"],"Next":"Denied","ResultPath":"$.errorInfo"}],"End":true}, ...}}

  expected (authorized leg): DescribeExecution status SUCCEEDED, a TaskSucceeded history event, and the message delivered to the subscribed SQS queue.
  actual: DescribeExecution status FAILED.
  Consequence: the unauthorized leg's required outcome (execution SUCCEEDED via the Catch to 'Denied', a TaskFailed event carrying SNS's AuthorizationError, nothing delivered to the queue) is indistinguishable from the authorized leg, because both legs end FAILED.

## Steps to Reproduce
1. Create an SNS topic and an SQS queue; set the queue's Policy to allow sns.amazonaws.com sqs:SendMessage with Condition aws:SourceArn = the topic ARN; Subscribe the queue to the topic with Protocol=sqs and wait until GetSubscriptionAttributes reports PendingConfirmation == "false".
2. Create an IAM role trusted by states.amazonaws.com and PutRolePolicy granting {"Effect":"Allow","Action":"sns:Publish","Resource":<topic arn>}; poll GetRolePolicy until it is readable.
3. CreateStateMachine (STANDARD) with the single Task state above: Resource "arn:aws:states:::sns:publish", Parameters {"TopicArn": <topic>, "Message.$": "$.message"}, and a Catch on States.ALL to a 'Denied' state.
4. StartExecution with input {"message": "..."} and poll DescribeExecution to a terminal status.
5. Observe status FAILED instead of SUCCEEDED, no TaskSucceeded event in GetExecutionHistory, and no message arriving on the subscribed SQS queue.
6. Now DeleteRolePolicy to remove the sns:Publish grant and start the identical execution again: the outcome is the same FAILED, so the authorized and unauthorized legs cannot be told apart.
