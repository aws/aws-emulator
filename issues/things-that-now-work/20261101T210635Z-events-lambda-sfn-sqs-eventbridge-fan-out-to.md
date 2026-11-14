Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs__stepfunctions.eventbridge-fanout-lambda-sqs-sfn events__lambda__sqs__stepfunctions.eventbridge-to-lambda-blocked-without-resource-policy events__lambda__sqs__stepfunctions.eventbridge-to-lambda-invokes-with-resource-policy events__lambda__sqs__stepfunctions.sqs-to-lambda-esm-handler-error-returns-message events__lambda__sqs__stepfunctions.sqs-to-lambda-esm-preserves-attributes
Issue-Key: events__lambda__sqs__stepfunctions.eventbridge-fanout-lambda-sqs-sfn|_receive_raw#22|Timeout|resp = ctx.client("sqs").receive_message(
Issue-Key: events__lambda__sqs__stepfunctions.eventbridge-to-lambda-blocked-without-resource-policy|_receive_and_drain#13|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs__stepfunctions.eventbridge-to-lambda-invokes-with-resource-policy|_receive_raw#8|Timeout|resp = ctx.client("sqs").receive_message(
Issue-Key: events__lambda__sqs__stepfunctions.sqs-to-lambda-esm-handler-error-returns-message|_wait_for_messages#9|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sqs__stepfunctions.sqs-to-lambda-esm-preserves-attributes|_wait_for_messages#8|Timeout|resp = sqs.receive_message(
Issue-Commit: 62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3

# events__lambda__sfn__sqs: EventBridge fan-out to Lambda and the SQS event-source mapping are both inert — 5 net-new causes never reach a verdict

> _Note: this issue was identified against the code base at commit `62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Five distinct root causes in this fixture group's eventing paths. Four hang to the harness budget (`Error: Check timed out after 60.0s`); one fails with a hard assertion.

Cause 1 of 5 — fan-out envelope: AWS: recorded behavior only (EventBridge's model defines PutEvents but no delivery semantics). One PutEvents event matching a rule with three targets reaches Lambda, SQS and Step Functions with an identical event envelope (same id/source/detail-type/detail in each target's payload). Emulator: no target is reached; the check times out at 60s.
Failing check: events__lambda__sfn__sqs.eventbridge-fanout-lambda-sqs-sfn.

Cause 2 of 5 — Lambda target blocked without a resource policy: AWS: recorded behavior only. With the lambda:InvokeFunction grant for events.amazonaws.com revoked, delivery to the Lambda target stops (the positive control on the identical rule/target having delivered first); granting it back on the same wiring restores delivery. Emulator: the positive control itself never delivers, so "blocked" cannot be separated from "never delivers"; the check times out at 60s.
Failing check: events__lambda__sfn__sqs.eventbridge-to-lambda-blocked-without-resource-policy.

Cause 3 of 5 — Lambda target invoked with the grant present: AWS: recorded behavior only. Once lambda:InvokeFunction is granted to events.amazonaws.com, the rule invokes the function for each matching event. Emulator: the function is never invoked; the check times out at 60s.
Failing check: events__lambda__sfn__sqs.eventbridge-to-lambda-invokes-with-resource-policy.

Cause 4 of 5 — SQS ESM failure semantics: AWS: recorded behavior only. A handler that raises must NOT have its source message deleted: the message returns to the queue after the visibility timeout. The check proves the handler actually ran by a side-channel marker queue the handler writes before raising. Emulator: the marker queue stays empty — `AssertionError: Expected 1 messages, got 0 within 45s` — so the ESM never invoked the handler at all; nothing about the redelivery contract is observable.
Failing check: events__lambda__sfn__sqs.sqs-to-lambda-esm-handler-error-returns-message.

Cause 5 of 5 — SQS ESM attribute fidelity: AWS: recorded behavior only. Records delivered by an SQS ESM preserve the message's attributes (messageAttributes with their DataType/Value, and the system attributes such as SentTimestamp/ApproximateReceiveCount) in the Lambda event. Emulator: no invocation occurs, so no record is ever delivered; the check times out at 60s.
Failing check: events__lambda__sfn__sqs.sqs-to-lambda-esm-preserves-attributes.

Why: every asynchronous consumer in this shape is dead. A client cannot build EventBridge fan-out, cannot use a Lambda resource policy to gate rule delivery (both the allowed and denied outcomes look identical), and cannot process SQS through an event-source mapping — including the at-least-once redelivery guarantee that makes failed batches safe. Worse, the negative checks pass for the wrong reason in a fixed emulator only if delivery works at all, so this masks authorization bugs.

Fix: the EventBridge rule-dispatch path (target invocation for Lambda/SQS/Step Functions, evaluating the Lambda resource policy for events.amazonaws.com) and the SQS event-source-mapping poller that must invoke the function and surface a handler error without deleting the message. Related open issues 20261029T182228Z-events-sfn-sqs-no-step-functions-execution-status-change-event-ever.md and 20261030T110426Z-events-lambda-sfn-sqs-createeventsourcemapping-s-under-privileged-role-refusal-says-the.md touch this group but carry none of these five checks.

Traces: the five check ids listed above.

## Evidence
Replay details: eventbridge-fanout-lambda-sqs-sfn, eventbridge-to-lambda-blocked-without-resource-policy, eventbridge-to-lambda-invokes-with-resource-policy and sqs-to-lambda-esm-preserves-attributes each report `Error: Check timed out after 60.0s` [mismatch]. sqs-to-lambda-esm-handler-error-returns-message reports `AssertionError: Expected 1 messages, got 0 within 45s` raised from _wait_for_messages on the handler's side-channel marker queue — i.e. the handler never ran. Expected in AWS: one event delivered identically to three targets; delivery toggling with the lambda:InvokeFunction grant; the ESM invoking the handler, the marker arriving, and the source message returning to the queue; message attributes preserved in the record.

## Steps to Reproduce
1. PutEvents one event onto a bus with a rule whose targets are a Lambda, an SQS queue and a state machine; compare the envelopes each target sees.
2. Remove the lambda:InvokeFunction grant for events.amazonaws.com and re-put the event (no delivery), then re-add it (delivery resumes).
3. CreateEventSourceMapping from an SQS queue to a Lambda whose handler writes to a second queue and then raises; send one message and poll both queues: the marker arrives and the source message becomes visible again.
4. Send a message with messageAttributes and inspect the delivered record's attributes.
5. Against the emulator: steps 1-3 (positive path) never deliver, so the checks hang at the 60s budget, and step 3's marker queue stays empty (AssertionError after 45s).
