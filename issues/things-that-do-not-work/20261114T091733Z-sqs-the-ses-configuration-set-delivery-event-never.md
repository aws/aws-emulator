Issue-Source: trace-sweep-divergence
Issue-Trace: sesv2-configuration-set-publishes-delivery-event-to-sns
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sesv2-configuration-set-publishes-delivery-event-to-sns
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sqs|GetQueueAttributes|2xx->2xx|$.Attributes.ApproximateNumberOfMessages
Issue-Key: sqs|ReceiveMessage|2xx->2xx|$
Issue-Commit: 194dd4ce0b6ed8c8e02c165143ff72645d766c54

# sqs: the SES configuration-set delivery event never reaches the subscribed queue, so the queue stays empty

> _Note: this issue was identified against the code base at commit `194dd4ce0b6ed8c8e02c165143ff72645d766c54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReceiveMessage

## Description
Two causes, both on service token sqs, both in fixture sesv2-configuration-set-publishes-delivery-event-to-sns. They share one fault: a message that an upstream service (SES -> SNS -> this queue) must have enqueued is never in the queue, so every sqs read of that queue reports an empty queue. The two causes are the two distinct sqs operations that misreport it; a fixer must close both.

Cause 1 of 2: GetQueueAttributes reports the queue as permanently empty (fixture sesv2-configuration-set-publishes-delivery-event-to-sns, step task#9).
AWS: Smithy (sqs): GetQueueAttributes is a 200 operation whose only output member is Attributes (QueueAttributeMap, wire name Attribute, flattened); the queue-depth attribute ApproximateNumberOfMessages is a decimal string count of the messages currently available for retrieval. Recorded behavior: after a send through a configuration set whose SNS event destination is subscribed by this queue, the recording polls this attribute and it becomes '1' — one event message is sitting in the queue. The requirement is the counting rule, not the literal: the attribute must count the messages the queue actually holds, so once one message has been enqueued by the SNS subscription it must read as at least one. No status or error diverges (200 both sides).
Emulator: the attribute stays '0' for the whole poll window ($.Attributes.ApproximateNumberOfMessages: expected '1', got '0'; 'poll: not in [1] after 40 attempt(s)'). The count itself is consistent with the emulator's state — nothing was ever enqueued — so what the client observes is a queue that is never fed.
Why: a client that wires SES -> SNS -> SQS and waits on queue depth to learn that an email event landed waits forever. Depth is the standard readiness signal for this pattern (and for every poller built on it), so the emulator reports 'nothing happened' for a scenario AWS completes, and any consumer-side logic (fan-out accounting, retry/backoff, DLQ thresholds) is unreachable.
Fix: not the GetQueueAttributes serializer — the attribute faithfully reports an empty queue. The missing component is the cross-service delivery leg that must enqueue the message: the sesv2 SendEmail handler's event-destination publication (for each event destination of the named configuration set with Enabled true and MatchingEventTypes selecting the event, publish the SES event object to the SnsDestination topic) and the SNS slice's sqs-protocol subscription delivery, which must call into the sqs queue state so the message is retrievable there.
Traces: sesv2-configuration-set-publishes-delivery-event-to-sns#task#9.

Cause 2 of 2: ReceiveMessage returns no Messages at all for the same queue (fixture sesv2-configuration-set-publishes-delivery-event-to-sns, step task#10).
AWS: Smithy (sqs): ReceiveMessage answers 200 with output member Messages (MessageList, wire name Message, flattened); each Message carries MessageId, ReceiptHandle, MD5OfBody and Body. Recorded behavior: the same poll returns exactly one Message whose Body is the SES event JSON delivered through the SNS subscription, with a ReceiptHandle and a MessageId (both per-run values, shape-asserted only, so no literal id or handle from the recording is a requirement) and an MD5OfBody that is the MD5 of whatever body is delivered. The requirement is that a message enqueued by the subscribed topic is returned by a receive on that queue.
Emulator: the response body is entirely empty ($: empty_body — expected one Messages entry, got {}). No status or error diverges (200 both sides); the divergence is the absent Messages list, i.e. the queue holds nothing to return.
Why: ReceiveMessage is how the consumer actually reads the event. With it empty, the delivery event (eventType, mail.messageId, mail.source) is never observable, so no bounce/complaint/delivery handling built on the SES -> SNS -> SQS pattern can be exercised or tested against the emulator, while the control plane continues to report a healthy topic, subscription and event destination.
Fix: same component as Cause 1 — the sesv2 configuration-set event-destination publish path and the SNS sqs-protocol subscription delivery into sqs queue state. Once a message is enqueued, ReceiveMessage must return it with a ReceiptHandle, a MessageId and MD5OfBody computed over the delivered body.
Traces: sesv2-configuration-set-publishes-delivery-event-to-sns#task#10.

Related open issues, none of which carries either of these two fixture steps' replay keys (the replay report marks both unowned), so none closes them: 20261109T232952Z-sesv2-sendemail-refuses-the-configuration-set-send-with.md covers the immediately upstream step task#8, where sesv2 SendEmail answers 400 instead of 200 and mints no MessageId — with the send refused nothing can be published, so that refusal must be fixed before this leg can be observed at all, but it is a sesv2 status defect and leaves this queue-delivery leg unverified; 20261102T025917Z-sesv2-sns-sqs-sendemail-publishes-no-event-to.md is a BEHAVIORAL issue (no fixture step) recording that SendEmail never reads a configuration set's event destinations and has no publish path, which is the same missing publication; 20261113T174325Z-sqs-eventbridgev2-subscriber-delivery-writes-an-sqs-message-body.md and 20261111T110051Z-sqs-2-net-new-causes-createqueue-publishes-a.md concern other sqs legs (EventBridgeV2 body shape, queue URLs and capacity) and not this one. Hypothesis (neither the model nor the recording states it): the emulator has no SES-event publication at all, so the SNS topic and this queue are never fed; whether the SNS->SQS subscription delivery itself works is not decided by these steps.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture sesv2-configuration-set-publishes-delivery-event-to-sns.

Step task#9 (sqs GetQueueAttributes), expected 200 / actual 200:
  $.Attributes.ApproximateNumberOfMessages: value_mismatch (expected '1', got '0')
  $.Attributes.ApproximateNumberOfMessages: poll: not in ['1'] after 40 attempt(s) (expected '1', got '0')
Expected: the attribute converges to a non-zero count once the SES event has been delivered to the queue. Actual: '0' through 40 polls.

Step task#10 (sqs ReceiveMessage), expected 200 / actual 200:
  $: empty_body (expected {'Messages': [{'Body': '<shaped-not-asserted>', 'MD5OfBody': '0af1f8afaff0628470385feaf85d34d0', 'MessageId': '<shaped-not-asserted>', 'ReceiptHandle': '<shaped-not-asserted>'}]}, got {})
Expected: one Message with Body, MD5OfBody, MessageId and ReceiptHandle (MessageId/ReceiptHandle/Body shape-asserted only; the MD5 is the digest of the delivered body, not a fixed literal). Actual: an empty response with no Messages member.

Neither step diverges on HTTP status (200 both sides) or on any error code or message; the divergence in both is the absence of the message the subscribed queue must hold.

Context in the same fixture: step task#8 (sesv2 SendEmail) answers 400 with no MessageId instead of 200, which is filed separately as 20261109T232952Z-sesv2-sendemail-refuses-the-configuration-set-send-with.md. The replay report lists task#9 and task#10 as owned by no open issue.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay wire fixture sesv2-configuration-set-publishes-delivery-event-to-sns from trace set simparity-wire-aws-v1.
2. Let the setup run: CreateTopic; CreateQueue; GetQueueAttributes(QueueArn); SetQueueAttributes with a policy allowing sns.amazonaws.com sqs:SendMessage from that topic; Subscribe(Protocol='sqs', Endpoint=<queue arn>); sesv2 CreateConfigurationSet and CreateConfigurationSetEventDestination with an Enabled SnsDestination on that topic whose MatchingEventTypes select the delivery event; establish the sending identity. All answer as recorded.
3. Step task#8: sesv2 SendEmail through that configuration set. (It currently answers 400; AWS answers 200 with a MessageId — see the separately filed sesv2 issue. Fix or bypass that refusal so a send is accepted.)
4. Step task#9: poll sqs GetQueueAttributes(QueueUrl=<queue>, AttributeNames=['ApproximateNumberOfMessages']). Observe it stays '0' across all 40 polls where AWS converges to a non-zero count.
5. Step task#10: call sqs ReceiveMessage on the same queue. Observe an empty 200 body with no Messages member, where AWS returns one Message carrying the SES event JSON Body plus MessageId, ReceiptHandle and MD5OfBody.
6. Re-replay both steps after wiring the SES event-destination publication into SNS and the SNS sqs-protocol delivery into sqs queue state.
