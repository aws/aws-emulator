Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives
Issue-Key: events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives|_receive#23|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# RDS DB-instance events never reach a matching EventBridge rule's SQS target

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One cause.

Cause 1 of 1: RDS lifecycle events are not published to the default event bus, so a rule whose pattern matches them never delivers and the near-miss rules cannot be graded against it.

AWS: recorded behavior only (event emission is not part of the Smithy model). RDS publishes DB-instance lifecycle events to the account's default EventBridge bus with `source` `aws.rds`, and a rule whose EventPattern matches that source/detail-type delivers the event to its SQS target, while rules whose patterns differ in one field receive nothing.

Emulator: the check's positive leg — the matching rule's queue receiving the RDS event — never happens; the check ends at its 60 s budget with nothing delivered, so neither the match nor the near-miss exclusions can be observed.

Why: a client that routes RDS lifecycle events through EventBridge (the documented way to react to DB availability, failover and maintenance) receives nothing at all; its rules look healthy and its queues stay empty.

Fix: the RDS control-plane state transitions' EventBridge publication (the component that emits `aws.rds` events onto the default bus).

Traces: events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives.

## Evidence
Expected: the RDS event reaches the queue targeted by the rule whose pattern matches, and does not reach the queues of the near-miss rules.
Actual (suite report): `Error: Check timed out after 60.0s` — the matching rule's queue never received the event, so the run never reached the near-miss assertions. The verdict is recorded as a mismatch for this check.

## Steps to Reproduce
1. Create an SQS queue with a policy allowing events.amazonaws.com, and an EventBridge rule on the default bus whose EventPattern matches `{"source": ["aws.rds"]}` plus sibling rules that differ in one field; target the queues.
2. Perform an RDS DB-instance lifecycle operation that AWS reports as an event.
3. Poll the matching rule's queue — no event arrives.
Replay: events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives.
