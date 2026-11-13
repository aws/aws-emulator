Issue-Source: trace-sweep-divergence
Issue-Trace: eventbridgev2-subscriber-delivers-published-event-to-sqs
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eventbridgev2-subscriber-delivers-published-event-to-sqs
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: sqs|ReceiveMessage|2xx->2xx|$.Messages[*].Body
Issue-Commit: 737fb93a0a7c6d2c5001b5b69dbb14ac16ed0b14

# EventBridgeV2 subscriber delivery writes an SQS message body outside the recorded shape

> _Note: this issue was identified against the code base at commit `737fb93a0a7c6d2c5001b5b69dbb14ac16ed0b14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReceiveMessage

## Description
=== Cause 1 of 1: the message an eventbridgev2 subscriber delivers to its SQS target has a body whose shape is not the recorded one ===
AWS: recorded behavior only for the delivered payload. Smithy (eventbridgev2) — Transformer.Type is an enum whose RAW member is documented "Deliver the raw event payload" and whose WITH_METADATA member is "Deliver the event with metadata envelope"; the subscriber under replay carries Transformer {Type: 'RAW'}. The recorded ReceiveMessage response for the target queue carries a Body that the fixture asserts by SHAPE only — i.e. the recorded delivery body contains at least one per-run generated value, so it is not the literal detail object the publisher sent. Hypothesis: the recorded body is the event envelope EventBridge writes around the published detail (version / id / detail-type / source / account / time / region / resources / detail), whose generated id and time are exactly what make the recorded body shape-asserted instead of literal.
Emulator: the delivered Body is exactly the detail the PutEvents entry carried ('{"orderId":"o-1001","quantity":3}') — a payload with no generated member, which does not satisfy the recorded body shape.
Why: a consumer that reads envelope members off the delivered message — source and detail-type to route, id to de-duplicate, time to order — finds none of them, so routing and de-duplication built on the AWS delivery contract silently stop working.
Fix: the delivery payload serializer of the eventbridgev2 subscriber dispatcher (the component that builds the message written to the target queue); the divergence is observed through SQS ReceiveMessage on the target queue, not in the SQS handlers themselves.
Traces: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#10.

Related open issue that does NOT carry this step's replay key: 20261113T094431Z-eventbridgev2-iam-sqs-9-net-new-causes-putevents.md (behavioral; its Cause 4 describes the recorded envelope keys but is keyed to behavioral check ids, and the envelope-pattern checks there never got past warm-up).

## Evidence
ReceiveMessage 200/200 on the subscriber's target queue: $.Messages[0].Body value_mismatch — expected the recorded body shape (asserted as '<shaped-not-asserted>', i.e. the recorded body carries per-run generated values), got '{"orderId":"o-1001","quantity":3}', the bare published detail with no generated member. Every other assertion on the step passed, so the message was delivered — only its body shape diverges.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture eventbridgev2-subscriber-delivers-published-event-to-sqs: CreateEventBus, CreateSubscriber with InvokeConfiguration {RoleArn, TargetArn: <sqs queue arn>, SqsParameters: {}} and Transformer {Type: RAW}, then PutEvents with one entry whose Detail is a JSON object.
3. Step task#10 — ReceiveMessage on the target queue and inspect Messages[0].Body.
