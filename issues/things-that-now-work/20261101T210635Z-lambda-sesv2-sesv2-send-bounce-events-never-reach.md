Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.sesv2-eventbridge-bounce-event-data-format lambda__sesv2.sesv2-eventbridge-send-event-lambda-invocation-positive
Issue-Key: lambda__sesv2.sesv2-eventbridge-bounce-event-data-format|probe#11|Timeout|return len(collected)
Issue-Key: lambda__sesv2.sesv2-eventbridge-send-event-lambda-invocation-positive|_drain_relay_events#12|Timeout|return collected
Issue-Commit: 62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3

# lambda__sesv2: SESv2 send/bounce events never reach EventBridge, so neither the event payload nor the rule's Lambda target is observable

> _Note: this issue was identified against the code base at commit `62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
Two distinct root causes in the SESv2 -> EventBridge -> Lambda path. Both end in `Error: Check timed out after 60.0s`.

Cause 1 of 2 — bounce event payload: AWS: recorded behavior only (the SESv2 model defines SendEmail and the configuration-set event destinations but not the event payload delivered to EventBridge). AWS behaviour exercised by the check: a message sent to a simulator/bouncing destination under a configuration set with an EventBridge event destination publishes an aws.ses event whose detail carries the bounce envelope (eventType Bounce, the mail object with messageId and the configuration-set name, and the bounce sub-type/recipients), matching the documented event-publishing data format.
Emulator: no event is ever delivered, so the payload cannot be compared; the check times out at 60s.
Failing check: lambda__sesv2.sesv2-eventbridge-bounce-event-data-format.

Cause 2 of 2 — send event invokes the rule's Lambda: AWS: recorded behavior only. A successful SendEmail under the same configuration set publishes the Send/Delivery event, and an EventBridge rule matching source aws.ses invokes its Lambda target with that event.
Emulator: the function is never invoked; the check times out at 60s.
Failing check: lambda__sesv2.sesv2-eventbridge-send-event-lambda-invocation-positive.

Why: email event publishing is how clients learn the fate of a message. With no event published (and no target invoked), bounces and deliveries are invisible: suppression-list automation, per-message auditing and delivery dashboards built on aws.ses events never run, and a bounced message is indistinguishable from a delivered one.

Fix: the SESv2 send path's event publication to the configuration set's EventBridge event destination (emitting the documented aws.ses event with its eventType/mail/bounce detail), plus the EventBridge rule dispatch that must invoke the Lambda target for those events.

Traces: the two check ids listed above.

## Evidence
Both failures report `Error: Check timed out after 60.0s` (replay verdicts [mismatch]).
- sesv2-eventbridge-bounce-event-data-format: expected an aws.ses event whose detail carries eventType Bounce plus the mail object (messageId, configuration-set name) and the bounce recipients; actual: no event delivered within 60s.
- sesv2-eventbridge-send-event-lambda-invocation-positive: expected the rule's Lambda target invoked with the Send/Delivery event; actual: no invocation within 60s.

## Steps to Reproduce
1. Create a verified identity and a configuration set with an EventBridge event destination enabled for send/delivery/bounce events.
2. Create an EventBridge rule matching source aws.ses with a Lambda target (and the lambda:InvokeFunction grant for events.amazonaws.com).
3. SendEmail to a bouncing destination under that configuration set, then to a deliverable one.
4. AWS publishes the aws.ses Bounce and Send/Delivery events and the rule invokes the function with each.
5. Against the emulator no event is published and the function is never invoked; both checks are killed at the 60s harness budget.
