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
Issue-Commit: 5530572bb5f665c51611ffda737da4493b534b4c

# sqs: an SES configuration-set delivery event never lands on the subscribed queue

> _Note: this issue was identified against the code base at commit `5530572bb5f665c51611ffda737da4493b534b4c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueueAttributes

## Description
Two net-new causes, both on service token sqs (the service the diverging steps are reported under), both observations of the same missing delivery path: an email sent through a configuration set that owns an SNS event destination produces no SES event, so the SNS topic's subscribed SQS queue stays empty forever. Both causes must be closed before this issue is resolved — a fixer must re-replay BOTH steps named below.

Cause 1 of 2 — GetQueueAttributes reports ApproximateNumberOfMessages '0' forever after the send (fixture: sesv2-configuration-set-publishes-delivery-event-to-sns#task#9).
AWS: Smithy (services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json): GetQueueAttributes(QueueUrl required, AttributeNames) answers 200 with Attributes, a QueueAttributeMap of string->string; ApproximateNumberOfMessages is the approximate number of messages available for retrieval, serialized as a decimal string. Recorded behavior: once the send through the configuration set is accepted, SES publishes the matching event to the destination's SNS topic, SNS fans it out to the subscribed queue, and this GetQueueAttributes on that queue reports ApproximateNumberOfMessages '1'. The requirement is the queue depth reaching 1, not any particular literal — the count is the number of events the scenario caused.
Emulator: the attribute never leaves '0' — $.Attributes.ApproximateNumberOfMessages: value_mismatch (expected '1', got '0'), and the harness polled 40 times before giving up ('poll: not in [\'1\'] after 40 attempt(s)'). The status is 200 on both sides, so the divergence is the queue depth and not the response shape or any error code.
Why: a client that wires a configuration set to an SNS event destination with an SQS subscriber and then waits for the Send/Delivery event waits forever and then reads an empty queue, so no delivery accounting, bounce handling or event-shape logic can be exercised even though every control-plane call in the scenario answered as recorded.
Fix: the SES event-publication path the configuration set owes its event destinations, plus the SNS->SQS fan-out that carries it — services/service-com-amazonaws-sesv2-simpleemailservice-v2 (the SendEmail handler and the configuration-set / event-destination records in its state) must, for each event destination of the named configuration set whose Enabled is true and whose MatchingEventTypes select the event, publish the SES event object to the destination's SnsDestination.TopicArn through the SNS slice, which must then enqueue it onto every subscribed queue in the sqs service's state so the queue's depth counter advances.
Traces: sesv2-configuration-set-publishes-delivery-event-to-sns#task#9.

Cause 2 of 2 — ReceiveMessage on the subscribed queue returns no messages (fixture: sesv2-configuration-set-publishes-delivery-event-to-sns#task#10).
AWS: Smithy (same sqs model): ReceiveMessage answers 200 with Messages, a MessageList of Message{MessageId, ReceiptHandle, MD5OfBody, Body}. Recorded behavior: the recording's ReceiveMessage against the same queue returns exactly one message, with MessageId, ReceiptHandle and Body all present; the recorded MD5OfBody literal is the digest of that run's event body (which embeds that run's message id and timestamps), so no literal digest, id, handle or body text from the recording is a requirement — the requirement is one message carrying the published SES event.
Emulator: the 200 body is empty — $: empty_body (expected {'Messages': [{'Body': '<shaped-not-asserted>', 'MD5OfBody': ..., 'MessageId': '<shaped-not-asserted>', 'ReceiptHandle': '<shaped-not-asserted>'}]}, got {}). An empty body is the correct answer for an empty queue, so the defect is that the queue is empty: the event was never enqueued.
Why: the consumer leg of the scenario receives nothing, so a client cannot read, parse or delete the email event it caused; every downstream assertion on the event body is unreachable.
Fix: same component as Cause 1 — once the SES event is published and fanned out, ReceiveMessage in the sqs service returns it as one Message with its MessageId, ReceiptHandle, MD5OfBody and Body.
Traces: sesv2-configuration-set-publishes-delivery-event-to-sns#task#10.

Related open issues, none of which closes these two steps (the replay marks neither step owned, so no filed issue carries their replay keys): 20261109T232952Z-sesv2-sendemail-refuses-the-configuration-set-send-with.md covers the upstream sesv2 SendEmail refusal at task#8 of this same fixture (400 instead of 200, no MessageId) and names task#9/task#10 only as its downstream effect; 20261102T025917Z-sesv2-sns-sqs-sendemail-publishes-no-event-to.md records the same inert publication path from the behavioral corpus (its own Enabled-gate check, not these wire steps); 20261109T155235Z-sqs-events-published-to-an-eventbridgev2-bus-are.md is the same symptom pair on a different producer (an EventBridgeV2 bus subscriber), and its replay keys are that fixture's. Note that the send at task#8 must first be accepted before these two steps can pass.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture sesv2-configuration-set-publishes-delivery-event-to-sns. The SNS topic, the SQS queue and its subscription, the configuration set and its SNS event destination, and the sending identity all answered as recorded; task#8 (sesv2 SendEmail) answered 400 instead of 200 (owned by 20261109T232952Z-sesv2-sendemail-refuses-the-configuration-set-send-with.md), and then:

task#9 sqs GetQueueAttributes (expected 200, got 200):
  $.Attributes.ApproximateNumberOfMessages: value_mismatch (expected '1', got '0')
  $.Attributes.ApproximateNumberOfMessages: poll: not in ['1'] after 40 attempt(s)

task#10 sqs ReceiveMessage (expected 200, got 200):
  $: empty_body (expected {'Messages': [{'Body': '<shaped-not-asserted>', 'MD5OfBody': '0af1f8afaff0628470385feaf85d34d0', 'MessageId': '<shaped-not-asserted>', 'ReceiptHandle': '<shaped-not-asserted>'}]}, got {})

Expected: the accepted send publishes its matching SES event to the destination's SNS topic, SNS delivers it to the subscribed queue, the queue reports ApproximateNumberOfMessages '1', and ReceiveMessage returns one Message with MessageId, ReceiptHandle, MD5OfBody and Body. Actual: the queue stays at depth 0 across 40 polls and ReceiveMessage returns an empty 200 body, so nothing was ever enqueued. The recorded MD5OfBody is the digest of that run's event body and is not itself a requirement.

Supporting code (per the related sesv2 issues, cited as the component, not as a patch): the SendEmail handler in services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/send_email.rs mints a MessageId and appends to the outbound log without ever reading the configuration set's event destinations, Enabled flag or MatchingEventTypes, and the crate has no publish path into the SNS slice — so nothing is ever delivered to the queue the fixture subscribes.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay wire fixture sesv2-configuration-set-publishes-delivery-event-to-sns from trace set simparity-wire-aws-v1.
2. Let the setup run: CreateTopic, CreateQueue, GetQueueAttributes(QueueArn), SetQueueAttributes with a policy allowing sns.amazonaws.com sqs:SendMessage from that topic, Subscribe(Protocol='sqs', Endpoint=<queue arn>), CreateConfigurationSet, CreateConfigurationSetEventDestination with an SnsDestination on that topic and Enabled=true, and establish the verified sending identity — all answer as recorded.
3. Step task#8: sesv2 SendEmail through that configuration set. AWS answers 200 with a MessageId (the emulator's 400 here is filed separately); the send must be accepted for the following two steps to be reachable.
4. Step task#9: poll sqs GetQueueAttributes on the queue for ApproximateNumberOfMessages — it stays '0' across 40 polls where AWS reports '1'.
5. Step task#10: call sqs ReceiveMessage on the queue — the 200 body is empty where AWS returns one Message carrying the published SES event, with MessageId, ReceiptHandle, MD5OfBody and Body.

Re-replay to confirm a fix: sesv2-configuration-set-publishes-delivery-event-to-sns#task#9 and sesv2-configuration-set-publishes-delivery-event-to-sns#task#10.
