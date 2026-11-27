Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sns__sqs.s3-sns-filter-policy-needs-messagebody-scope-without-attributes
Issue-Key: s3__sns__sqs.s3-sns-filter-policy-needs-messagebody-scope-without-attributes|_first_records#16|Timeout|time.sleep(poll_s)
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# An S3 notification to SNS with a MessageBody-scoped filter policy is never delivered to the topic's queue subscriber

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
One cause.

Cause 1 of 1: an S3 event notification published to an SNS topic is not matched by a subscription whose FilterPolicyScope is `MessageBody`, so nothing reaches the subscriber.

AWS: an S3 bucket notification published to SNS carries the event JSON as the notification's `Message` and sets no message attributes. A subscription filter policy therefore only matches when `FilterPolicyScope` is `MessageBody` (Smithy: com.amazonaws.sns Subscribe/SetSubscriptionAttributes accept the `FilterPolicy` and `FilterPolicyScope` subscription attributes); with that scope, a policy over the event body's fields matches and the notification is delivered to the queue subscriber, while an attribute-scoped policy matches nothing.

Emulator: the check never observes the delivery it grades; it ends at its 60 s budget with nothing on the queue, so the MessageBody-scoped match does not happen.

Why: a client that filters S3 notifications at the topic — the documented way to fan one bucket's events out to several consumers — receives no events at all, with no error on either the S3 or the SNS side.

Fix: the SNS publish-time filter evaluation for `FilterPolicyScope: MessageBody` (and the S3 notification publisher that must put the event JSON in the notification's Message).

Traces: s3__sns__sqs.s3-sns-filter-policy-needs-messagebody-scope-without-attributes.

## Evidence
Expected: with FilterPolicyScope=MessageBody and a policy over the S3 event body, the notification reaches the subscribed queue; an attribute-scoped policy on the same topic delivers nothing.
Actual (suite report): `Error: Check timed out after 60.0s` — the MessageBody-scoped subscriber never received the notification. The verdict for this check is recorded as a mismatch.

## Steps to Reproduce
1. Create a topic, a queue subscribed to it with `FilterPolicyScope=MessageBody` and a FilterPolicy over fields of the S3 event body, plus the topic policy allowing s3.amazonaws.com.
2. Configure the bucket's notification configuration to publish object-created events to the topic.
3. Put an object and poll the queue — nothing arrives.
Replay: s3__sns__sqs.s3-sns-filter-policy-needs-messagebody-scope-without-attributes.
