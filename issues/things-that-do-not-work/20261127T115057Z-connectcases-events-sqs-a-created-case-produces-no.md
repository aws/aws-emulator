Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcases__events__sqs.case-created-event-reaches-queue-with-included-fields connectcases__events__sqs.case-created-while-event-stream-disabled-not-delivered
Issue-Key: connectcases__events__sqs.case-created-event-reaches-queue-with-included-fields|_events#25|Timeout|got = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: connectcases__events__sqs.case-created-while-event-stream-disabled-not-delivered|_case_ids#23|Timeout|got = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# connectcases__events__sqs: a created case produces no EventBridge CaseCreated event — both case-event-stream checks exhaust their budget

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCase

## Description
Cause 1 of 1: Connect Cases does not publish its case event stream to EventBridge, so neither the positive delivery nor the event-stream-disabled negative can be established; both checks run out of their 60s budget.

AWS: with a case event configuration enabled for the domain (PutCaseEventConfiguration with IncludedData), CreateCase publishes an event to the default bus with source `aws.cases`, detail-type naming the case event (CaseCreated) and a detail carrying the case id, the template/domain and the fields the configuration includes; a rule on that source delivers it to its target. With the event configuration disabled, no event is published. Recorded behavior only (the event stream contents are documented for Amazon Connect Cases, not modeled in Smithy).
Emulator: neither check reaches a verdict within 60.0s — the awaited case event never arrives on the rule's queue, and the disabled-stream case cannot be distinguished because the enabled case never delivers either.
Why: a client building case automation (routing, notification, downstream sync) on the Cases event stream receives nothing, and the privacy-relevant difference between an enabled and a disabled event configuration is unobservable.
Fix: the connectcases CreateCase handler's event publication (and the PutCaseEventConfiguration state that gates it) — it must put a `aws.cases` CaseCreated event with the configured included data onto the default event bus.
Traces: connectcases__events__sqs.case-created-event-reaches-queue-with-included-fields, connectcases__events__sqs.case-created-while-event-stream-disabled-not-delivered.

## Evidence
Both checks: `Error: Check timed out after 60.0s` — the rule's queue never received a `aws.cases` CaseCreated event after CreateCase with an enabled case event configuration.

## Steps to Reproduce
1. connectcases CreateDomain, CreateField/CreateTemplate, PutCaseEventConfiguration(EventBridge={Enabled: true, IncludedData: ...}).
2. events PutRule on the default bus with pattern {source:["aws.cases"]} and PutTargets naming an SQS queue.
3. connectcases CreateCase.
4. Observe no message on the queue.
