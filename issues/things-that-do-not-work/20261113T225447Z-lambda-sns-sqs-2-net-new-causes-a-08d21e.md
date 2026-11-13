Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.dead-letter-topic-relays-the-original-event-with-lambda-error-attributes lambda__sns__sqs.on-success-destination-topic-publishes-the-invocation-record-as-the-message
Issue-Key: lambda__sns__sqs.dead-letter-topic-relays-the-original-event-with-lambda-error-attributes|run#16|AssertionError|assert sorted(attrs) == ["ErrorCode", "ErrorMessage", "RequestID"], (
Issue-Key: lambda__sns__sqs.on-success-destination-topic-publishes-the-invocation-record-as-the-message|run#17|AssertionError|assert rc.get("functionArn") == f"{fn['arn']}:$LATEST", f"functionArn: {rc.get('functionArn')!r}"
Issue-Commit: 23d7c3ca3e68cb70ca93342274fc19d1394baa26

# lambda__sns__sqs: 2 net-new causes — a dead-letter SNS relay carries no RequestID/ErrorCode/ErrorMessage message attributes, and the async invocation record's functionArn is not qualified with :$LATEST

> _Note: this issue was identified against the code base at commit `23d7c3ca3e68cb70ca93342274fc19d1394baa26`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
TWO net-new root causes of service `lambda__sns__sqs`, grounded in the failing checks in trace_ids. Neither check is marked ALREADY FILED or OWNED in the replay report. Context: the open issue 20261107T014900Z-lambda-sns-sqs-async-destinations-and-dead-letter.md (its Cause 1) describes these two paths NEVER DELIVERING (both checks previously timed out while polling the queue); delivery now happens, so its replay keys no longer match and the two causes below are what the delivered payloads get wrong. BOTH must be closed before this issue is submitted.

=== Cause 1 of 2: the dead-letter SNS publish carries no message attributes at all ===
AWS: Smithy (lambda UpdateFunctionConfiguration -> DeadLetterConfig.TargetArn) models the configuration; the payload is AWS-documented behavior (Lambda Developer Guide, asynchronous invocation / dead-letter queues). When an asynchronous invocation exhausts its retries and the function has a DeadLetterConfig naming an SNS topic, Lambda publishes the ORIGINAL event as the Message and attaches exactly three message attributes: `RequestID` (DataType String — the request id of the async invoke), `ErrorCode` (DataType Number — the invocation's error code) and `ErrorMessage` (DataType String — the handler's error message, truncated to 1 KB). The attribute NAMES and their three DataTypes are the fixed contract; the values come from the failed invocation. Through a non-raw SNS subscription they arrive in the envelope's MessageAttributes; through a raw subscription as SQS message attributes.
Emulator: the notification arrives (the Message is relayed) but the attribute set is EMPTY — `the relayed attributes are []`.
Why: a dead-letter consumer identifies and correlates the failure solely from those attributes: RequestID is the only link back to the invocation's logs, ErrorCode/ErrorMessage are the only statement of WHY the event died (the Message is just the original event, which says nothing about the failure). With no attributes, a DLQ consumer cannot triage or route, and attribute-based SNS filter policies on a dead-letter topic match nothing.
Fix: the Lambda asynchronous-invocation pipeline's dead-letter publish — attach RequestID (String), ErrorCode (Number) and ErrorMessage (String) from the failed invocation to the SNS Publish (and the equivalent SQS message attributes for a queue target).
Traces: lambda__sns__sqs.dead-letter-topic-relays-the-original-event-with-lambda-error-attributes.

=== Cause 2 of 2: the invocation record published to an OnSuccess destination reports an unqualified functionArn ===
AWS: Smithy (lambda PutFunctionEventInvokeConfig -> DestinationConfig.OnSuccess) models the configuration; the record shape is AWS-documented behavior (Lambda Developer Guide, asynchronous invocation — "invocation record"). The record published to the destination carries `version`, `timestamp`, `requestContext` {`requestId`, `functionArn`, `condition`, `approximateInvokeCount`}, `requestPayload` and `responsePayload`. `functionArn` is the QUALIFIED ARN of the function version that ran: for an invoke with no Qualifier that is the function's ARN with the `:$LATEST` suffix appended — fixed suffix, the ARN itself taken from the request.
Emulator: `functionArn` is the UNQUALIFIED function ARN — e.g. `'arn:aws:lambda:<region>:<account>:function:<name>'` where AWS writes `'arn:aws:lambda:<region>:<account>:function:<name>:$LATEST'` (region/account/name are per-run values; the missing `:$LATEST` qualifier is the defect). The record's other asserted fields up to that point (condition Success, requestId) were reached without failing.
Why: the invocation record is the audit/replay artifact for async invocations, and consumers parse `functionArn` to learn WHICH version produced the result — that is the whole reason it is qualified. A consumer that re-invokes or attributes the record by that ARN loses the version, and equality checks against the qualified ARN AWS emits fail.
Fix: the invocation-record serializer in the Lambda asynchronous-invocation pipeline — emit requestContext.functionArn qualified by the version/alias the invocation used (`:$LATEST` when the invoke carried no Qualifier).
Traces: lambda__sns__sqs.on-success-destination-topic-publishes-the-invocation-record-as-the-message.

## Evidence
Cause 1 — lambda__sns__sqs.dead-letter-topic-relays-the-original-event-with-lambda-error-attributes [mismatch]:
  File "check_lambda__sns__sqs_dead_letter_topic_relays_the_original_event_with_lambda_error_attributes.py", line 56, in run — assert sorted(attrs) == ["ErrorCode", "ErrorMessage", "RequestID"]
  AssertionError: the relayed attributes are []
Expected: the dead-letter notification's attributes are exactly RequestID (String), ErrorCode (Number) and ErrorMessage (String); actual: no attributes at all.

Cause 2 — lambda__sns__sqs.on-success-destination-topic-publishes-the-invocation-record-as-the-message [mismatch]:
  File "check_lambda__sns__sqs_on_success_destination_topic_publishes_the_invocation_record_as_the_message.py", line 56, in run — assert rc.get("functionArn") == f"{fn['arn']}:$LATEST"
  AssertionError: functionArn: 'arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-fn-29948d8347a6'
Expected: that same ARN with the ':$LATEST' qualifier appended (the region/account/function name are per-run values).

## Steps to Reproduce
Cause 1: create an SNS topic with an SQS queue subscribed (queue policy allowing sns.amazonaws.com); create a function whose execution role may sns:Publish to the topic and whose handler raises; UpdateFunctionConfiguration(DeadLetterConfig={'TargetArn': <topic arn>}); Invoke with InvocationType='Event'; ReceiveMessage from the queue and read the notification's MessageAttributes — they are empty where AWS carries RequestID/ErrorCode/ErrorMessage.
Cause 2: same topic/queue wiring; PutFunctionEventInvokeConfig(MaximumRetryAttempts=0, DestinationConfig={'OnSuccess': {'Destination': <topic arn>}}); Invoke with InvocationType='Event' on a handler that returns a value; ReceiveMessage and parse the envelope's Message as the invocation record — requestContext.functionArn lacks the ':$LATEST' qualifier.
Then re-replay both check ids in trace_ids.
