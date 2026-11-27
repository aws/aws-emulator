Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__logs__sqs.sqs-esm-record-message-attribute-carries-list-value-members
Issue-Key: iam__lambda__logs__sqs.sqs-esm-record-message-attribute-carries-list-value-members|run#21|AssertionError|assert attrs == want, f"record messageAttributes {attrs!r}, expected {want!r}"
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# SQS event-source-mapping records omit the messageAttributes list members stringListValues and binaryListValues

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One cause.

Cause 1 of 1: each SQS record the event-source mapping hands a Lambda function carries only `dataType` and `stringValue` in `messageAttributes`, dropping the always-present empty `stringListValues` and `binaryListValues` members.

AWS: recorded behavior only for the invocation event shape (the Smithy model describes SendMessage.MessageAttributes via MessageBodyAttributeMap / MessageAttributeValue, whose StringListValues and BinaryListValues members exist but are not populated over the wire). The SQS event Lambda receives renders every user message attribute as an object with `stringValue`, `dataType` and empty `stringListValues` and `binaryListValues` arrays, so a handler can index those members unconditionally.

Emulator: the record's attribute reads `{'dataType': 'String', 'stringValue': 'myValue'}` where `{'binaryListValues': [], 'dataType': 'String', 'stringListValues': [], 'stringValue': 'myValue'}` is required.

Why: a handler written against the documented record shape — typed SDK event models in Java/.NET/Go deserialize these members, and hand-written handlers index them directly — raises a KeyError/NullReferenceException on a perfectly ordinary message, so the whole batch fails and the messages are retried until they dead-letter.

Fix: the SQS event-source-mapping record serializer that builds the Lambda invocation event's `messageAttributes` entries.

The same defect is reported under the lambda__sqs heading as `lambda__sqs.sqs-esm-event-message-attributes-carry-list-value-members`; a single issue cannot span two service headings, so that one is filed with the lambda__sqs bundle.

Traces: iam__lambda__logs__sqs.sqs-esm-record-message-attribute-carries-list-value-members.

## Evidence
Expected: `{'myAttribute': {'binaryListValues': [], 'dataType': 'String', 'stringListValues': [], 'stringValue': 'myValue'}}`.
Actual: `{'myAttribute': {'dataType': 'String', 'stringValue': 'myValue'}}`. The attribute name and value come from the request; the four member names and the empty arrays are the fixed shape.

## Steps to Reproduce
1. Create a function that logs the raw SQS event and an event-source mapping from a standard queue.
2. SendMessage with one String message attribute.
3. Read the logged record's `messageAttributes` entry — `stringListValues` and `binaryListValues` are absent.
Replay: iam__lambda__logs__sqs.sqs-esm-record-message-attribute-carries-list-value-members.
