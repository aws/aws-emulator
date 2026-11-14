Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns__sqs.lambda-sdk-publish-to-sns
Issue-Key: events__lambda__sns__sqs.lambda-sdk-publish-to-sns|_invoke_lambda#11|Timeout|return result
Issue-Commit: 6bba0ea93be6d4db7a2caa629527597bd59ab221

# events__lambda__sns__sqs: an in-function boto3 sns:Publish never completes end-to-end, so the granted Lambda->SNS->SQS happy path reaches no verdict

> _Note: this issue was identified against the code base at commit `6bba0ea93be6d4db7a2caa629527597bd59ab221`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: a python3.12 Lambda whose execution role carries an explicit `sns:Publish` grant on the topic calls `boto3.client('sns').publish(TopicArn=..., Message='from-lambda-sdk')` from its handler. Neither the invoke nor the resulting delivery ever succeeds: the check exhausts its whole budget and is cut off with `Check timed out after 60.0s`.

AWS: Smithy: `Publish` takes `TopicArn` + `Message` and returns `PublishResponse.MessageId`; it is authorized against the caller's identity policy and the topic policy, and a caller whose role allows `sns:Publish` on that topic gets a 200 with a MessageId (errors are modeled as `AuthorizationErrorException` 403, `NotFoundException` 404, `InvalidParameterException` 400 — none of which applies to a granted caller). Lambda `Invoke` with `InvocationType=RequestResponse` returns `StatusCode=200` and NO `FunctionError` member when the handler returns normally. Recorded behavior for the fan-out: a `sqs`-protocol subscription whose queue policy allows `sns:SendMessage` from the topic receives the notification, and the envelope's `Message` member equals the published Message verbatim.

Emulator: the check reaches no verdict inside 60 s. Its own bounded loop re-invokes the function until `Invoke` comes back without a `FunctionError` (the loop exists only to absorb the IAM role-grant propagation window) and then polls the subscribed queue; one of those two never converges — either the in-function SDK `sns.publish` keeps failing (so `Invoke` keeps reporting a FunctionError) or the publish succeeds and the notification never reaches the subscribed queue. The runner cuts the check off with `Check timed out after 60.0s` rather than any assertion message.

Why: the single most common serverless shape — a function that publishes to a topic that fans out to a queue — silently never delivers. A client sees either a handler exception for a call its role plainly permits, or a 200 with a MessageId for a message no subscriber ever receives; in both cases the downstream consumer is starved and there is no error to act on.

Fix: the in-function AWS SDK/credential plumbing of the Lambda execution environment and its authorization of `sns:Publish` against the execution role, plus the SNS Publish handler's `sqs`-protocol subscription delivery path (and the SQS queue-policy evaluation that admits it).

Related but NOT covering this check: open issues 20261106T133436Z-lambda-sns-sqs-in-function-aws-sdk-calls.md and 20261105T081727Z-lambda-sns-sqs-the-emulator-s-python-lambda.md describe in-function SDK defects under the `lambda__sns__sqs` suite; neither carries this check's replay key, and the suite does not report this check as owned by them.

Traces: events__lambda__sns__sqs.lambda-sdk-publish-to-sns.

## Evidence
Expected: `Invoke(FunctionName=<fn>, InvocationType='RequestResponse', Payload='{}')` -> `StatusCode=200`, no `FunctionError`, payload `{"ok": true}`, because the execution role's inline policy allows `sns:Publish` on the topic ARN; then `ReceiveMessage` on the subscribed queue returns exactly 1 message whose JSON body has `Message == "from-lambda-sdk"`.

Actual: no verdict — the runner reports `Check timed out after 60.0s`. The check's bounded re-invoke loop (60 s, 5 s interval, described as "Lambda's sns:Publish succeeding once its role grant propagates") plus the 45 s `_wait_for_messages` poll on the subscribed queue never converge, so neither the `"error" not in invoke_result` assertion nor the `len(msgs) == 1` / `body["Message"] == "from-lambda-sdk"` assertions are ever reached.

(The generated function, topic and queue names/ARNs are per-run and are not part of the requirement; the requirement is that a granted in-function `sns:Publish` returns a MessageId and that the published Message arrives verbatim in the `sqs`-protocol subscriber.)

## Steps to Reproduce
1. sns: CreateTopic. sqs: CreateQueue, GetQueueAttributes(QueueArn).
2. sqs: SetQueueAttributes with a Policy allowing Principal Service `sns.amazonaws.com` to `sqs:SendMessage` on the queue ARN, conditioned ArnEquals aws:SourceArn = the topic ARN.
3. sns: Subscribe(TopicArn, Protocol='sqs', Endpoint=<queue ARN>).
4. iam: CreateRole trusting `lambda.amazonaws.com`; PutRolePolicy with an inline policy allowing logs:* plus `sns:Publish` on the topic ARN.
5. lambda: CreateFunction (runtime python3.12, handler `lambda_function.lambda_handler`) whose code is `import boto3; sns = boto3.client("sns")` and whose handler calls `sns.publish(TopicArn=<topic arn>, Message="from-lambda-sdk")` then returns `{"ok": True}`. Wait for State=Active.
6. lambda: Invoke(InvocationType='RequestResponse', Payload='{}') repeatedly until it comes back with no FunctionError.
7. sqs: ReceiveMessage(MaxNumberOfMessages=1, WaitTimeSeconds=5) in a loop for 45 s.
8. Observed: steps 6-7 never converge; the check is cut off at 60 s with `Check timed out after 60.0s`.

Replay: events__lambda__sns__sqs.lambda-sdk-publish-to-sns
