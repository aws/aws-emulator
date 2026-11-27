Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__sns.publish-over-a-raised-topic-maximum-message-size-rejected
Issue-Key: cloudwatch__lambda__sns|Publish|InvalidParameter
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# cloudwatch__lambda__sns: SNS Publish refuses a message that fits the topic's raised MaximumMessageSize with InvalidParameter 'Invalid parameter: Message too long'

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: a topic created with the MaximumMessageSize attribute raised above the 256 KiB default stores and reports the raised value, but Publish still enforces the default, so a message within the topic's own limit is refused. Failing check: cloudwatch__lambda__sns.publish-over-a-raised-topic-maximum-message-size-rejected.

AWS: Smithy (services/service-com-amazonaws-sns-amazonsimplenotificationservice) CreateTopicInput$Attributes documents 'MaximumMessageSize – The maximum size, in bytes, of a message that can be published to the topic. Valid values are 1024 to 1048576 (1 MiB). The default is 262144 (256 KiB). A topic with a MaximumMessageSize above 256 KiB must have 100 or fewer subscriptions, and each subscription must be an Amazon SQS, Amazon Data Firehose, or Lambda subscription.' So on a topic whose MaximumMessageSize is 1048576: a 1 048 576-byte Message is accepted and Publish returns a MessageId; a 1 048 577-byte Message is refused with HTTP 400, error code InvalidParameter and the message 'Invalid parameter: Message too long' (fixed text; Publish models InvalidParameterException with awsQuery code InvalidParameter, httpError 400); and a message above the default but within the raised limit is delivered intact to a Lambda subscription as an async event.

Emulator: get_topic_attributes reads MaximumMessageSize back as the raised value (that assertion passes), but Publish of a message within that limit is refused with the very error AWS reserves for exceeding it — botocore InvalidParameterException, code InvalidParameter, message 'Invalid parameter: Message too long'. The refusal escapes the check's own expected-refusal guard, which is only wrapped around the deliberately over-limit publish, so the limit is being evaluated against the 256 KiB default rather than the topic's attribute. Everything the check asserts afterwards (the over-limit refusal's exact code and message, the 600 000-byte delivery reaching the subscribed function with a matching SHA-256, and AsyncEventsReceived matching Invocations minus the direct invokes) is never reached and is not claimed as diverging here.

Why: the raised-message-size feature is unusable. A client that configures a topic for large payloads (the documented SQS/Firehose/Lambda-only configuration) is told its message is too long for a message the topic is configured to accept, and the attribute it just read back is not the limit actually enforced — the control plane and the data plane disagree.

Fix: the Publish handler's size validation must take the limit from the topic's own MaximumMessageSize attribute in state (defaulting to 262144 when unset) instead of a hard-coded default, keeping the refusal for a message that genuinely exceeds that limit as HTTP 400 InvalidParameter 'Invalid parameter: Message too long'. CreateTopic/SetTopicAttributes validation of the attribute's 1024..1048576 range is unaffected.

Traces: cloudwatch__lambda__sns.publish-over-a-raised-topic-maximum-message-size-rejected (the raised limit in the scenario is 1048576 and the delivered body is 600000 bytes; both are the check's parameters, and the requirement is that the enforced limit equals the topic's attribute).

## Evidence
Suite: service cloudwatch__lambda__sns scored 1/2 (50%), 1 failed; not OWNED, not ALREADY FILED.

[error] cloudwatch__lambda__sns.publish-over-a-raised-topic-maximum-message-size-rejected
  botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameter) when calling the Publish operation: Invalid parameter: Message too long
  Control that passed: CreateTopic with Attributes {MaximumMessageSize: '1048576'} succeeded and GetTopicAttributes read MaximumMessageSize back as '1048576'.
  Expected (AWS): Publish of a Message of exactly the topic's MaximumMessageSize returns 200 with a MessageId; only MaximumMessageSize+1 is refused 400 InvalidParameter 'Invalid parameter: Message too long'.
  Actual (emulator): Publish within the topic's configured limit is refused 400 InvalidParameter 'Invalid parameter: Message too long'.

Topic names, function names and message ids of the recorded run are per-run values; the requirement is that the enforced limit is the topic's MaximumMessageSize attribute and that the refusal wording/status is used only when that limit is exceeded.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. sns:CreateTopic with Attributes {MaximumMessageSize: '1048576'}; sns:GetTopicAttributes -> MaximumMessageSize == '1048576'.
3. sns:Publish to that topic with a Message of exactly 1048576 bytes.
   -> AWS: HTTP 200 with a MessageId.
   -> Emulator: HTTP 400, InvalidParameter, 'Invalid parameter: Message too long'.
4. sns:Publish a Message of 1048577 bytes -> AWS and emulator both must answer 400 InvalidParameter 'Invalid parameter: Message too long'.
5. Subscribe a Lambda function to the topic (lambda:AddPermission for sns.amazonaws.com) and publish a 600000-byte message -> AWS delivers it intact as an async event (the handler sees the same length and digest) and AsyncEventsReceived counts it.
6. Re-replay the check in trace_ids to confirm the fix; it then continues into the over-limit refusal, the delivery digest and the AsyncEventsReceived/Invocations legs.
